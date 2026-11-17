//! `scan`: static route analysis -> endpoint YAML files.
//!
//! Each framework module exposes `pub fn scan(project_root: &Path, files: &[PathBuf]) -> Result<Vec<Route>>`.
//! The core in this file collects source files, runs the framework scanner,
//! then creates new endpoint files and refreshes only the `source:` block of
//! existing ones (text based, so everything humans / AI wrote stays
//! byte-identical).

pub mod express;
pub mod fastapi;
pub mod go;
pub mod hono;
pub mod nextjs;
mod ts_common;

use crate::cli::ScanArgs;
use crate::config::Project;
use crate::model::{EndpointDef, SourceInfo, FRAMEWORKS, SCHEMA_VERSION};
use crate::naming;
use crate::report::{ScanEntry, ScanOptions, ScanReport, ScanStatus, REPORT_VERSION};
use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// A route found in the source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Upper-case HTTP method.
    pub method: String,
    /// Normalized OpenAPI-style path (see `naming::normalize_path`).
    pub path: String,
    /// Path relative to the project root, `/`-separated.
    pub file: String,
    /// 1-based line of the route registration / handler.
    pub line: u32,
    pub handler: Option<String>,
    /// Query parameter names when statically known (FastAPI signatures, ...).
    pub query_params: Vec<String>,
    /// `framework` value written into `source.framework`.
    pub framework: String,
}

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "build",
    ".next",
    "out",
    "target",
    "venv",
    ".venv",
    "__pycache__",
    "vendor",
    "coverage",
];

const SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/niti-lab/ajiro-api/main/schema/endpoint.schema.json";

/// Render a string as a YAML scalar (quoted only when needed).
pub(crate) fn yaml_scalar(s: &str) -> String {
    serde_yaml_rust2::to_string(s)
        .map(|t| t.trim_end().to_string())
        .unwrap_or_else(|_| format!("{:?}", s))
}

fn family(framework: &str) -> &str {
    if framework.starts_with("go-") {
        "go"
    } else {
        framework
    }
}

fn is_excluded(rel: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|p| {
        let p = p.replace('*', "");
        let p = p.trim_matches('/');
        !p.is_empty() && rel.contains(p)
    })
}

fn abs_path(project: &Project, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        project.root.join(p)
    }
}

fn collect_files(project: &Project, opts: &ScanOptions) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = if opts.paths.is_empty() {
        project.source_dirs()
    } else {
        opts.paths.iter().map(|p| abs_path(project, p)).collect()
    };
    let mut files = Vec::new();
    for r in roots {
        if r.is_file() {
            files.push(r);
            continue;
        }
        let walker = walkdir::WalkDir::new(&r)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                if e.file_type().is_dir()
                    && (SKIP_DIRS.contains(&name.as_ref()) || name.starts_with('.'))
                {
                    return false;
                }
                !is_excluded(&project.rel(e.path()), &project.config.exclude)
            });
        for e in walker
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            files.push(e.into_path());
        }
    }
    files.sort();
    files.dedup();
    files
}

fn render_source_children(s: &SourceInfo) -> Vec<String> {
    let mut v = vec![
        format!("  framework: {}", yaml_scalar(&s.framework)),
        format!("  file: {}", yaml_scalar(&s.file)),
        format!("  line: {}", s.line),
    ];
    if let Some(h) = &s.handler {
        v.push(format!("  handler: {}", yaml_scalar(h)));
    }
    v
}

fn render_new_file(def_id: &str, route: &Route, source: &SourceInfo) -> String {
    let mut out = String::new();
    out.push_str(&format!("# yaml-language-server: $schema={}\n", SCHEMA_URL));
    out.push_str(&format!("version: {}\n", SCHEMA_VERSION));
    out.push_str(&format!("id: {}\n", yaml_scalar(def_id)));
    out.push_str("request:\n");
    out.push_str(&format!("  method: {}\n", route.method));
    out.push_str(&format!("  path: {}\n", yaml_scalar(&route.path)));
    if !route.query_params.is_empty() {
        out.push_str("  query:\n");
        for q in &route.query_params {
            out.push_str(&format!("    {}: null\n", yaml_scalar(q)));
        }
    }
    out.push_str("# written by ajiro-api scan \u{2014} do not edit\n");
    out.push_str("source:\n");
    for l in render_source_children(source) {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str("# written by humans and AI \u{2014} scan never touches this\n");
    out.push_str("tests: []\n");
    out
}

fn strip_eol(l: &str) -> &str {
    l.trim_end_matches(['\n', '\r'])
}

/// Replace (or insert) the top-level `source:` block; everything else is
/// kept byte-identical. Returns `None` if the file cannot be edited safely.
fn update_source_text(text: &str, new: &SourceInfo, had_source: bool) -> Option<String> {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let children: Vec<String> = render_source_children(new);
    let pos = lines
        .iter()
        .position(|l| strip_eol(l).starts_with("source:"));
    let mut out = String::with_capacity(text.len() + 128);
    match pos {
        Some(i) => {
            let head = strip_eol(lines[i]);
            let value = head["source:".len()..]
                .split('#')
                .next()
                .unwrap_or("")
                .trim();
            let mut last = i + 1;
            for (j, l) in lines.iter().enumerate().skip(i + 1) {
                let l = strip_eol(l);
                if l.trim().is_empty() {
                    continue;
                }
                if l.starts_with(' ') || l.starts_with('\t') {
                    last = j + 1;
                } else {
                    break;
                }
            }
            for l in &lines[..i] {
                out.push_str(l);
            }
            if value.is_empty() {
                out.push_str(head);
            } else {
                out.push_str("source:");
            }
            out.push_str(eol);
            for c in &children {
                out.push_str(c);
                out.push_str(eol);
            }
            for l in &lines[last..] {
                out.push_str(l);
            }
        }
        None => {
            if had_source {
                return None;
            }
            let at = match lines
                .iter()
                .position(|l| strip_eol(l).starts_with("tests:"))
            {
                Some(mut t) => {
                    while t > 0 && strip_eol(lines[t - 1]).starts_with('#') {
                        t -= 1;
                    }
                    t
                }
                None => lines.len(),
            };
            for l in &lines[..at] {
                out.push_str(l);
            }
            if at == lines.len() && !out.is_empty() && !out.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str("# written by ajiro-api scan \u{2014} do not edit");
            out.push_str(eol);
            out.push_str("source:");
            out.push_str(eol);
            for c in &children {
                out.push_str(c);
                out.push_str(eol);
            }
            for l in &lines[at..] {
                out.push_str(l);
            }
        }
    }
    Some(out)
}

/// Whether the project depends on `@hono/zod-openapi` (package.json, or an
/// import in a scanned file). Only called when a scan found nothing.
fn uses_hono_openapi(root: &Path, files: &[PathBuf]) -> bool {
    let needle = "zod-openapi";
    if std::fs::read_to_string(root.join("package.json")).is_ok_and(|t| t.contains(needle)) {
        return true;
    }
    files
        .iter()
        .filter(|f| ts_common::is_ts_source(f))
        .any(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains(needle)))
}

fn run_framework(
    framework: &str,
    project: &Project,
    files: &[PathBuf],
) -> Result<(Vec<Route>, Vec<String>)> {
    let root = project.root.as_path();
    Ok(match framework {
        "nextjs" => nextjs::scan_with_warnings(root, files)?,
        "hono" => hono::scan_with_warnings(root, files)?,
        "express" => express::scan_with_warnings(root, files)?,
        "fastapi" => (fastapi::scan(root, files)?, vec![]),
        "go-nethttp" | "go-chi" | "go-echo" => (go::scan(root, files)?, vec![]),
        other => bail!("unsupported framework `{}`", other),
    })
}

pub fn scan_project(project: &Project, opts: &ScanOptions) -> Result<ScanReport> {
    let started = Instant::now();
    let framework = match opts
        .framework
        .clone()
        .or_else(|| project.config.framework.clone())
        .or_else(|| crate::init::detect_framework(&project.root))
    {
        Some(f) => f,
        None => bail!(
            "could not detect the framework. Pass --framework <{}> or set `framework` in ajiro-api.yaml",
            FRAMEWORKS.join("|")
        ),
    };
    if !FRAMEWORKS.contains(&framework.as_str()) {
        bail!(
            "unknown framework `{}` (supported: {})",
            framework,
            FRAMEWORKS.join(", ")
        );
    }

    let files = collect_files(project, opts);
    let (mut routes, mut warnings) = run_framework(&framework, project, &files)?;
    routes.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    routes.dedup_by(|b, a| a.path == b.path && a.method == b.method);
    if routes.is_empty() {
        let mut msg = format!(
            "no {} routes found in {} scanned files",
            framework,
            files.len()
        );
        if framework == "hono" && uses_hono_openapi(&project.root, &files) {
            msg.push_str(
                ". This project uses @hono/zod-openapi: if the app serves an OpenAPI document, \
                 `ajiro-api openapi import <openapi.json>` can create the endpoint files from it instead",
            );
        }
        warnings.push(msg);
    }

    // Existing endpoint files.
    let (existing, load_errors) = crate::project::load_endpoints(project);
    for e in &load_errors {
        warnings.push(format!("{}: {}", e.file, e.message));
    }
    let mut by_key: HashMap<(String, String), usize> = HashMap::new();
    for (i, e) in existing.iter().enumerate() {
        let key = (
            e.def.request.method.to_ascii_uppercase(),
            naming::normalize_path(&e.def.request.path),
        );
        by_key.entry(key).or_insert(i);
    }
    let mut taken_ids: HashSet<String> = existing.iter().map(|e| e.def.id.clone()).collect();
    let mut taken_paths: HashSet<PathBuf> = existing.iter().map(|e| e.path.clone()).collect();
    let api_dir = project.api_dir();

    let mut matched: HashSet<usize> = HashSet::new();
    let mut entries = Vec::new();
    let (mut created, mut updated, mut unchanged) = (0, 0, 0);
    let write = !(opts.dry_run || opts.check);

    for route in &routes {
        let source = SourceInfo {
            framework: if route.framework.is_empty() {
                framework.clone()
            } else {
                route.framework.clone()
            },
            file: route.file.clone(),
            line: route.line,
            handler: route.handler.clone(),
        };
        let key = (route.method.clone(), route.path.clone());
        if let Some(&idx) = by_key.get(&key) {
            matched.insert(idx);
            let e = &existing[idx];
            let mut status = ScanStatus::Unchanged;
            if e.def.source.as_ref() != Some(&source) {
                let text = std::fs::read_to_string(&e.path)
                    .with_context(|| format!("cannot read {}", e.rel_path))?;
                match update_source_text(&text, &source, e.def.source.is_some()) {
                    Some(new_text) if new_text != text => {
                        if write {
                            std::fs::write(&e.path, new_text)
                                .with_context(|| format!("cannot write {}", e.rel_path))?;
                        }
                        status = ScanStatus::Updated;
                    }
                    Some(_) => {}
                    None => warnings.push(format!(
                        "{}: cannot update `source:` automatically; edit it by hand",
                        e.rel_path
                    )),
                }
            }
            match status {
                ScanStatus::Updated => updated += 1,
                _ => unchanged += 1,
            }
            entries.push(ScanEntry {
                id: e.def.id.clone(),
                method: route.method.clone(),
                path: route.path.clone(),
                file: e.rel_path.clone(),
                source,
                status,
            });
        } else {
            let rel = naming::endpoint_rel_path(&route.method, &route.path);
            let mut path = api_dir.join(&rel);
            let mut n = 2;
            while taken_paths.contains(&path) || path.exists() {
                let stem = rel
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("endpoint");
                path = api_dir.join(rel.with_file_name(format!("{}-{}.yaml", stem, n)));
                n += 1;
            }
            let id =
                naming::unique_id(&naming::endpoint_id(&route.method, &route.path), &taken_ids);
            taken_ids.insert(id.clone());
            taken_paths.insert(path.clone());
            let text = render_new_file(&id, route, &source);
            serde_yaml_rust2::from_str::<EndpointDef>(&text)
                .context("internal error: generated invalid YAML")?;
            if write {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &text)
                    .with_context(|| format!("cannot write {}", path.display()))?;
            }
            created += 1;
            entries.push(ScanEntry {
                id,
                method: route.method.clone(),
                path: route.path.clone(),
                file: project.rel(&path),
                source,
                status: ScanStatus::Created,
            });
        }
    }

    // Stale: files written by scan whose route is gone.
    let scoped: Vec<String> = opts
        .paths
        .iter()
        .map(|p| project.rel(&abs_path(project, p)))
        .collect();
    let mut stale = Vec::new();
    for (i, e) in existing.iter().enumerate() {
        if matched.contains(&i) {
            continue;
        }
        let Some(src) = &e.def.source else { continue };
        if family(&src.framework) != family(&framework) {
            continue;
        }
        if !scoped.is_empty() {
            let f = src.file.trim_start_matches("./");
            let inside = scoped.iter().any(|s| {
                let s = s.trim_end_matches('/');
                s.is_empty() || s == "." || f == s || f.starts_with(&format!("{}/", s))
            });
            if !inside {
                continue;
            }
        }
        stale.push(e.rel_path.clone());
    }
    stale.sort();

    Ok(ScanReport {
        report_version: REPORT_VERSION,
        framework,
        endpoints: entries,
        created,
        updated,
        unchanged,
        stale,
        warnings,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

pub fn run_cli(dir: &Path, args: &ScanArgs) -> Result<i32> {
    let project = Project::discover_or_default(dir)?;
    let base = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let opts = ScanOptions {
        framework: args.framework.clone(),
        paths: args
            .paths
            .iter()
            .map(|p| {
                if p.is_absolute() {
                    p.clone()
                } else {
                    base.join(p)
                }
            })
            .collect(),
        dry_run: args.dry_run,
        check: args.check,
    };
    let report = scan_project(&project, &opts)?;
    let drift = report.created + report.updated > 0;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report, args.dry_run || args.check);
        if args.check && drift {
            eprintln!("endpoint files are out of date: run `ajiro-api scan` and commit the result");
        }
    }
    Ok(if args.check && drift {
        crate::exit::FAILURE
    } else {
        crate::exit::OK
    })
}

fn print_human(r: &ScanReport, read_only: bool) {
    for e in &r.endpoints {
        let mark = match e.status {
            ScanStatus::Created => "+",
            ScanStatus::Updated => "~",
            ScanStatus::Unchanged => continue,
        };
        println!("{} {} {}  {}", mark, e.method, e.path, e.file);
    }
    println!(
        "{}: {} created, {} updated, {} unchanged ({} endpoints, {} ms){}",
        r.framework,
        r.created,
        r.updated,
        r.unchanged,
        r.endpoints.len(),
        r.duration_ms,
        if read_only { " [nothing written]" } else { "" }
    );
    for w in &r.warnings {
        println!("warning: {}", w);
    }
    if !r.stale.is_empty() {
        println!(
            "stale: {} file(s) whose route no longer exists in the code (never deleted automatically; remove them if the endpoint is gone):",
            r.stale.len()
        );
        for s in &r.stale {
            println!("  - {}", s);
        }
    }
}
