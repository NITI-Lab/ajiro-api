//! `scan`: static route analysis -> endpoint YAML files.
//!
//! Each framework module exposes `pub fn scan(project_root: &Path, files: &[PathBuf]) -> Result<Vec<Route>>`.
//! The core in this file collects source files, runs the framework scanner,
//! then creates new endpoint files and refreshes only the `source:` block of
//! existing ones (text based, so everything humans / AI wrote stays
//! byte-identical).

pub(crate) mod dircache;
pub mod express;
pub mod fastapi;
mod gitignore;
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

/// Test code is not application code: skipped below the scanned directory.
fn is_test_dir(name: &str) -> bool {
    matches!(name, "tests" | "test" | "__tests__")
}

fn is_test_file(name: &str) -> bool {
    name == "conftest.py"
        || (name.starts_with("test_") && name.ends_with(".py"))
        || name.ends_with("_test.py")
        || name.ends_with("_test.go")
        || name.contains(".test.")
        || name.contains(".spec.")
}

fn collect_files(project: &Project, opts: &ScanOptions, framework: &str) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = if opts.paths.is_empty() {
        project.source_dirs()
    } else {
        opts.paths.iter().map(|p| abs_path(project, p)).collect()
    };
    let mut files = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for r in roots {
        if r.is_file() {
            if seen.insert(r.clone()) {
                files.push(r);
            }
            continue;
        }
        let exclude = project.config.exclude.clone();
        let proj = project.clone();
        let is_next = framework == "nextjs";
        let mut git = gitignore::GitFilter::discover(&r);
        let walker = walkdir::WalkDir::new(&r)
            .follow_links(false)
            .into_iter()
            .filter_entry(move |e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                let rel = proj.rel(e.path());
                let is_dir = e.file_type().is_dir();
                if is_dir {
                    if SKIP_DIRS.contains(&name.as_ref()) || name.starts_with('.') {
                        return false;
                    }
                    // A Next.js route directory may legitimately be named `test`.
                    let route_dir = is_next && rel.split('/').any(|c| c == "app" || c == "pages");
                    if is_test_dir(&name) && !(route_dir && name != "__tests__") {
                        return false;
                    }
                } else if is_test_file(&name) {
                    return false;
                }
                if is_excluded(&rel, &exclude) {
                    return false;
                }
                !git.as_mut().is_some_and(|g| g.is_ignored(e.path(), is_dir))
            });
        // Sorted within each source, sources kept in the configured order:
        // the first source wins when two routes collide.
        let mut batch: Vec<PathBuf> = walker
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect();
        batch.sort();
        for f in batch {
            if seen.insert(f.clone()) {
                files.push(f);
            }
        }
    }
    files
}

/// Clean `file` for output: `/`-separated, no duplicate slashes, relative to
/// the project root when inside it.
fn clean_file(project: &Project, file: &str) -> String {
    let f = file.replace('\\', "/");
    let p = Path::new(&f);
    let f = match p.strip_prefix(&project.root) {
        Ok(r) if p.is_absolute() => r.to_string_lossy().replace('\\', "/"),
        _ => f,
    };
    let lead = f.starts_with('/');
    let parts: Vec<&str> = f
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    format!("{}{}", if lead { "/" } else { "" }, parts.join("/"))
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
        "fastapi" => fastapi::scan_with_warnings(root, files)?,
        "go-nethttp" | "go-chi" | "go-echo" => (go::scan(root, files)?, vec![]),
        other => bail!("unsupported framework `{}`", other),
    })
}

/// Whether `f` has an extension the `framework` scanner parses.
fn is_source_file(f: &Path, framework: &str) -> bool {
    let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
    match framework {
        "fastapi" => ext == "py",
        "go-nethttp" | "go-chi" | "go-echo" => ext == "go",
        _ => matches!(
            ext,
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts"
        ),
    }
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
            "could not detect the framework. Pass --framework <{}> or set `framework` in ajiro-api.yaml. {}",
            FRAMEWORKS.join("|"),
            crate::init::no_framework_message(&crate::init::detect(&project.root))
        ),
    };
    if !FRAMEWORKS.contains(&framework.as_str()) {
        bail!(
            "unknown framework `{}` (supported: {})",
            framework,
            FRAMEWORKS.join(", ")
        );
    }

    dircache::clear();
    let files = collect_files(project, opts, &framework);
    let (mut routes, mut warnings) = run_framework(&framework, project, &files)?;
    for r in &mut routes {
        r.file = clean_file(project, &r.file);
    }
    // One endpoint per method + path: the first route in scanner order (which
    // follows `sources` order) wins; every dropped duplicate is reported.
    let framework_warnings = warnings.len();
    let mut first: HashMap<(String, String), usize> = HashMap::new();
    let mut kept: Vec<Route> = Vec::with_capacity(routes.len());
    for r in routes {
        match first.get(&(r.method.clone(), r.path.clone())) {
            Some(&i) => {
                let k = &kept[i];
                // The framework scanner may already have explained this one
                // (FastAPI reports which registration shadows which).
                let explained = warnings[..framework_warnings]
                    .iter()
                    .any(|w| w.starts_with(&format!("duplicate route {} {}:", r.method, r.path)));
                if !explained && (k.file.as_str(), k.line) != (r.file.as_str(), r.line) {
                    warnings.push(format!(
                        "duplicate route {} {}: {}:{} ignored, keeping {}:{}",
                        r.method, r.path, r.file, r.line, k.file, k.line
                    ));
                }
            }
            None => {
                first.insert((r.method.clone(), r.path.clone()), kept.len());
                kept.push(r);
            }
        }
    }
    let mut routes = kept;
    routes.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    if routes.is_empty() {
        let parsed = files
            .iter()
            .filter(|f| is_source_file(f, &framework))
            .count();
        let mut msg = format!(
            "no {} routes found in {} scanned source file(s) ({} files in total, others are not code). \
             The project may use a framework ajiro-api does not support (supported: {}); \
             check `sources`/`exclude` in ajiro-api.yaml and --framework",
            framework,
            parsed,
            files.len(),
            FRAMEWORKS.join(", ")
        );
        let unsupported = crate::init::detect(&project.root).unsupported;
        if !unsupported.is_empty() {
            msg.push_str(&format!(
                ". This project looks like {}, which is not supported",
                unsupported.join(" / ")
            ));
        }
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
    // Match existing files by method + path, keeping a trailing slash (FastAPI
    // serves `/items/` and `/items` as different routes). A file whose exact
    // path no scanned route has also matches with the slash ignored, so a
    // hand-written `/x/` still pairs with a scanned `/x` (and vice versa).
    let mut by_key: HashMap<(String, String), usize> = HashMap::new();
    for (i, e) in existing.iter().enumerate() {
        let key = (
            e.def.request.method.to_ascii_uppercase(),
            naming::normalize_path_keep_slash(&e.def.request.path),
        );
        by_key.entry(key).or_insert(i);
    }
    let scanned_keys: HashSet<(String, String)> = routes
        .iter()
        .map(|r| (r.method.clone(), r.path.clone()))
        .collect();
    let loose = |p: &str| naming::normalize_path(p);
    let loose_scanned: HashMap<(String, String), (String, String)> = routes
        .iter()
        .filter(|r| r.path != loose(&r.path))
        .map(|r| {
            (
                (r.method.clone(), loose(&r.path)),
                (r.method.clone(), r.path.clone()),
            )
        })
        .collect();
    for (i, e) in existing.iter().enumerate() {
        let method = e.def.request.method.to_ascii_uppercase();
        let exact = naming::normalize_path_keep_slash(&e.def.request.path);
        if scanned_keys.contains(&(method.clone(), exact.clone())) {
            continue;
        }
        let lk = (method.clone(), loose(&exact));
        // existing `/x/` -> scanned `/x`
        if scanned_keys.contains(&lk) {
            by_key.entry(lk.clone()).or_insert(i);
        }
        // existing `/x` -> scanned `/x/`
        if let Some(sk) = loose_scanned.get(&lk) {
            by_key.entry(sk.clone()).or_insert(i);
        }
    }
    let taken_ids: HashSet<String> = existing.iter().map(|e| e.def.id.clone()).collect();
    let mut taken_paths: HashSet<PathBuf> = existing.iter().map(|e| e.path.clone()).collect();
    let api_dir = project.api_dir();

    let mut matched: HashSet<usize> = HashSet::new();
    let mut entries = Vec::new();
    let (mut created, mut updated, mut unchanged) = (0, 0, 0);
    let mut line_only_count = 0;
    let write = !(opts.dry_run || opts.check);

    // Ids for new files are assigned for the whole batch at once so that they
    // do not depend on scan order (see `naming::assign_ids`).
    let new_idx: Vec<usize> = (0..routes.len())
        .filter(|&i| !by_key.contains_key(&(routes[i].method.clone(), routes[i].path.clone())))
        .collect();
    let new_pairs: Vec<(String, String)> = new_idx
        .iter()
        .map(|&i| (routes[i].method.clone(), routes[i].path.clone()))
        .collect();
    let new_ids: HashMap<usize, String> = new_idx
        .iter()
        .copied()
        .zip(naming::assign_ids(&new_pairs, &taken_ids))
        .collect();
    let new_rels: HashMap<usize, PathBuf> = new_idx
        .iter()
        .map(|&i| {
            (
                i,
                naming::endpoint_rel_path(&routes[i].method, &routes[i].path),
            )
        })
        .collect();

    // File-name collisions get `-2`, `-3` in (path, method) order, not scan order.
    let mut file_order = new_idx.clone();
    file_order.sort_by(|&a, &b| {
        (&routes[a].path, &routes[a].method, a).cmp(&(&routes[b].path, &routes[b].method, b))
    });
    let mut new_rels = new_rels;
    {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for &i in &file_order {
            let rel = new_rels[&i].clone();
            let mut cand = rel.clone();
            let mut n = 2;
            while seen.contains(&cand) {
                let stem = rel
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("endpoint");
                cand = rel.with_file_name(format!("{}-{}.yaml", stem, n));
                n += 1;
            }
            seen.insert(cand.clone());
            new_rels.insert(i, cand);
        }
    }

    for (ri, route) in routes.iter().enumerate() {
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
            let line_only = e.def.source.as_ref().is_some_and(|old| {
                old.line != source.line
                    && SourceInfo {
                        line: source.line,
                        ..old.clone()
                    } == source
            });
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
            let line_only = line_only && status == ScanStatus::Updated;
            match status {
                ScanStatus::Updated => updated += 1,
                _ => unchanged += 1,
            }
            if line_only {
                line_only_count += 1;
            }
            entries.push(ScanEntry {
                id: e.def.id.clone(),
                method: route.method.clone(),
                path: route.path.clone(),
                file: e.rel_path.clone(),
                source,
                status,
                line_only,
            });
        } else {
            let rel = new_rels[&ri].clone();
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
            let id = new_ids[&ri].clone();
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
                line_only: false,
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
        line_only: line_only_count,
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
    let drift = report_has_drift(&report, args.ignore_lines);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report, args.dry_run || args.check);
        if args.check && drift {
            eprintln!("endpoint files are out of date: run `ajiro-api scan` and commit the result");
        } else if args.check && args.ignore_lines && report.line_only > 0 {
            eprintln!(
                "only `source.line` is out of date in {} file(s); not counted as drift (run `ajiro-api scan` to refresh them)",
                report.line_only
            );
        }
    }
    // `--check` must not pass vacuously: nothing found and nothing on disk
    // means the scan saw no project at all (wrong framework or sources).
    let vacuous = args.check && report.endpoints.is_empty() && report.stale.is_empty();
    if vacuous && !args.json {
        eprintln!(
            "scan --check found no routes and there are no endpoint files: nothing was checked"
        );
    }
    Ok(if (args.check && drift) || vacuous {
        crate::exit::FAILURE
    } else {
        crate::exit::OK
    })
}

/// Drift for `scan --check`: a new endpoint or a changed `source`. With
/// `ignore_lines`, files whose only change is `source.line` do not count:
/// any edit above a route shifts its line, which would otherwise fail CI on
/// unrelated changes.
pub fn report_has_drift(r: &ScanReport, ignore_lines: bool) -> bool {
    let updated = if ignore_lines {
        r.updated.saturating_sub(r.line_only)
    } else {
        r.updated
    };
    r.created + updated > 0
}

fn print_human(r: &ScanReport, read_only: bool) {
    for e in &r.endpoints {
        let mark = match e.status {
            ScanStatus::Created => "+",
            ScanStatus::Updated => "~",
            ScanStatus::Unchanged => continue,
        };
        let note = if e.line_only {
            format!("  (line {} only)", e.source.line)
        } else {
            String::new()
        };
        println!("{} {} {}  {}{}", mark, e.method, e.path, e.file, note);
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
