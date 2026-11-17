//! `init`: create ajiro-api.yaml + api/, detect the framework.

use crate::cli::InitArgs;
use crate::model::{ProjectConfig, CONFIG_FILE, FRAMEWORKS};
use crate::scan::yaml_scalar;
use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::Path;

fn read(root: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(root.join(name)).ok()
}

/// Whether the directory tree below `dir` contains a Next.js `route.*` file.
fn has_route_handlers(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(e.file_type().is_dir() && (n == "node_modules" || n.starts_with('.')))
        })
        .filter_map(|e| e.ok())
        .any(|e| {
            let n = e.file_name().to_string_lossy();
            e.file_type().is_file()
                && matches!(
                    n.as_ref(),
                    "route.ts" | "route.js" | "route.tsx" | "route.jsx"
                )
        })
}

fn node_deps(root: &Path) -> Option<HashSet<String>> {
    let text = read(root, "package.json")?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut deps = HashSet::new();
    for k in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(o) = v.get(k).and_then(|x| x.as_object()) {
            deps.extend(o.keys().cloned());
        }
    }
    Some(deps)
}

/// Outcome of framework detection: the supported framework (if any) plus
/// notes that explain a missing or rejected detection.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    pub framework: Option<String>,
    /// Recognisable stacks ajiro-api cannot scan (`NestJS`, `Flask`, ...).
    pub unsupported: Vec<String>,
    /// Why a dependency was not taken as the framework.
    pub notes: Vec<String>,
}

/// Whether a source file under `root` creates a Hono app (`new Hono(`).
fn has_hono_app(root: &Path) -> bool {
    let skip = ["node_modules", "dist", "build", ".git", ".next", ".output"];
    walkdir::WalkDir::new(root)
        .max_depth(5)
        .into_iter()
        .filter_entry(|e| !skip.iter().any(|s| e.file_name() == *s))
        .filter_map(|e| e.ok())
        .take(5000)
        .filter(|e| {
            e.file_type().is_file()
                && matches!(
                    e.path().extension().and_then(|x| x.to_str()),
                    Some("ts" | "tsx" | "js" | "jsx" | "mjs" | "mts")
                )
                && e.metadata().map(|m| m.len() < 512 * 1024).unwrap_or(false)
        })
        .any(|e| {
            std::fs::read_to_string(e.path())
                .map(|t| t.contains("new Hono(") || t.contains("new OpenAPIHono("))
                .unwrap_or(false)
        })
}

fn has_wrangler(root: &Path) -> bool {
    ["wrangler.toml", "wrangler.json", "wrangler.jsonc"]
        .iter()
        .any(|f| root.join(f).is_file())
}

/// A package that depends on `hono` but is a browser app (Vite, React, ...)
/// with no server entry: Hono is only used as a typed client (`hono/client`).
fn hono_is_client_only(root: &Path, deps: &HashSet<String>) -> bool {
    let frontend = ["vite", "react", "react-dom", "vue", "svelte", "solid-js"]
        .iter()
        .any(|d| deps.contains(*d))
        || deps
            .iter()
            .any(|d| d.starts_with("@tanstack/") && d.contains("router"));
    if !frontend {
        return false;
    }
    let server_dep = deps.contains("@hono/node-server")
        || deps.contains("@hono/vite-dev-server")
        || deps.contains("@hono/zod-openapi")
        || deps.contains("@cloudflare/workers-types") && has_wrangler(root);
    !(server_dep || has_wrangler(root) || has_hono_app(root))
}

fn python_text(root: &Path) -> String {
    let mut py = String::new();
    for f in ["pyproject.toml", "Pipfile"] {
        py.extend(read(root, f));
        py.push('\n');
    }
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.filter_map(|e| e.ok()) {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("requirements") && n.ends_with(".txt") {
                py.extend(std::fs::read_to_string(e.path()).ok());
                py.push('\n');
            }
        }
    }
    py.to_ascii_lowercase()
}

/// Whether `name` appears as a Python dependency in `text` (lower-cased).
fn py_has(text: &str, name: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim().trim_matches(|c| c == '"' || c == '\'' || c == ',');
        l.strip_prefix(name).is_some_and(|rest| {
            !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                && !rest.starts_with('-')
        })
    }) || text.contains(&format!("\"{}", name))
        || text.contains(&format!("'{}", name))
}

/// Recognisable stacks that ajiro-api does not support.
fn unsupported_stacks(root: &Path, deps: Option<&HashSet<String>>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Some(deps) = deps {
        let has = |d: &str| deps.contains(d);
        if has("@nestjs/core") {
            v.push("NestJS (@nestjs/core)".into());
        }
        if has("fastify") {
            v.push("Fastify".into());
        }
        if has("koa") {
            v.push("Koa".into());
        }
        if has("@aws-amplify/backend") {
            v.push("AWS Amplify Gen2 (@aws-amplify/backend)".into());
        }
        if has("@types/google-apps-script") || has("@google/clasp") {
            v.push("Google Apps Script".into());
        }
    }
    if root.join("appsscript.json").is_file() || root.join(".clasp.json").is_file() {
        let n = "Google Apps Script";
        if !v.iter().any(|x| x == n) {
            v.push(n.into());
        }
    }
    let hono = deps.is_some_and(|d| d.contains("hono"));
    let pages_dep =
        deps.is_some_and(|d| d.contains("wrangler") || d.contains("@cloudflare/next-on-pages"));
    if root.join("functions").is_dir() && (has_wrangler(root) || pages_dep) && !hono {
        v.push("Cloudflare Pages Functions (functions/ directory)".into());
    }
    let py = python_text(root);
    if py_has(&py, "flask") {
        v.push("Flask".into());
    }
    if py_has(&py, "django") {
        v.push("Django".into());
    }
    v
}

/// Detect the framework used in `root` and explain what was seen.
pub fn detect(root: &Path) -> Detection {
    let mut d = Detection::default();
    let deps = node_deps(root);
    d.unsupported = unsupported_stacks(root, deps.as_ref());
    if let Some(deps) = &deps {
        let next = deps.contains("next");
        let hono = deps.contains("hono") || deps.iter().any(|d| d.starts_with("@hono/"));
        // NestJS runs on Express/Fastify but its routes are decorators.
        let nest = deps.contains("@nestjs/core");
        let express = deps.contains("express") && !nest;
        let next_routes = ["app", "src/app"]
            .iter()
            .any(|d| has_route_handlers(&root.join(d)))
            || root.join("pages/api").is_dir()
            || root.join("src/pages/api").is_dir();
        if next && next_routes {
            d.framework = Some("nextjs".into());
            return d;
        }
        if hono {
            if hono_is_client_only(root, deps) {
                d.notes.push(
                    "`hono` is a dependency, but this looks like a browser app (Vite/React/...) \
                     with no Hono server (no wrangler config, @hono/node-server or `new Hono()`): \
                     Hono is probably only used as a client (`hono/client`), so it was not picked"
                        .into(),
                );
            } else {
                d.framework = Some("hono".into());
                return d;
            }
        }
        if express {
            d.framework = Some("express".into());
            return d;
        }
        if next && !nest {
            d.framework = Some("nextjs".into());
            return d;
        }
    }
    let py = python_text(root);
    if py.contains("fastapi") {
        d.framework = Some("fastapi".into());
        return d;
    }
    if let Some(gomod) = read(root, "go.mod") {
        d.framework = Some(
            if gomod.contains("github.com/go-chi/chi") {
                "go-chi"
            } else if gomod.contains("github.com/labstack/echo") {
                "go-echo"
            } else {
                "go-nethttp"
            }
            .into(),
        );
    }
    d
}

/// Detect the framework used in `root` (package.json deps, pyproject/requirements, go.mod).
pub fn detect_framework(root: &Path) -> Option<String> {
    detect(root).framework
}

/// Human-readable explanation for "no supported framework detected", naming
/// known unsupported stacks. Shared by `init` and `scan`.
pub fn no_framework_message(d: &Detection) -> String {
    let mut m = format!(
        "no supported framework detected (supported: {}).",
        FRAMEWORKS.join(", ")
    );
    if !d.unsupported.is_empty() {
        m.push_str(&format!(
            " This project looks like {}, which ajiro-api cannot scan yet.",
            d.unsupported.join(" / ")
        ));
    }
    for n in &d.notes {
        m.push(' ');
        m.push_str(n);
        m.push('.');
    }
    m.push_str(&format!(
        " If it uses one of the supported frameworks, set `framework` in {} (or pass --framework <name>); \
         otherwise write the endpoint files by hand or import them with `ajiro-api openapi import`.",
        CONFIG_FILE
    ));
    m
}

fn default_base_url(root: &Path, framework: Option<&str>) -> String {
    if let Some((port, _)) = detect_port(root) {
        return format!("http://localhost:{}", port);
    }
    let port = match framework {
        Some("fastapi") => 8000,
        Some(f) if f.starts_with("go-") => 8080,
        Some("hono")
            if ["wrangler.toml", "wrangler.json", "wrangler.jsonc"]
                .iter()
                .any(|f| root.join(f).is_file()) =>
        {
            8787
        }
        _ => 3000,
    };
    format!("http://localhost:{}", port)
}

/// Find the port the dev server listens on, with the file it came from.
///
/// Looks at (in order of trust): `PORT=` in `.env.example` / `.env`
/// (only that line is read), `--port` / `-p` in package.json scripts and
/// Dockerfiles, then source code: `serve({ port: 8000 })`,
/// `process.env.PORT ?? 8000`, `.listen(8000)`, `uvicorn.run(..., port=8000)`,
/// `ListenAndServe(":8080")`, `e.Start(":8080")`. Entry-point files win over
/// other files; within a tier the most frequent port wins.
pub fn detect_port(root: &Path) -> Option<(u16, String)> {
    use regex::Regex;
    use std::collections::HashMap;

    let valid = |p: &str| p.parse::<u16>().ok().filter(|p| *p >= 80);

    // 1. dotenv files: PORT=8000
    let env_re = Regex::new(r"(?m)^\s*(?:export\s+)?PORT\s*=\s*[\x22']?(\d{2,5})").unwrap();
    for f in [".env.example", ".env.sample", ".env.local", ".env"] {
        if let Ok(t) = std::fs::read_to_string(root.join(f)) {
            if let Some(p) = env_re.captures(&t).and_then(|c| valid(&c[1])) {
                return Some((p, f.to_string()));
            }
        }
    }
    // 2. scripts: "dev": "next dev -p 4000", uvicorn --port 8000
    let flag_re = Regex::new(r"(?:--port[ =]|\s-p\s+)(\d{2,5})\b").unwrap();
    for f in [
        "package.json",
        "Dockerfile",
        "docker-compose.yml",
        "compose.yaml",
        "Makefile",
        "Procfile",
    ] {
        if let Ok(t) = std::fs::read_to_string(root.join(f)) {
            if let Some(p) = flag_re.captures(&t).and_then(|c| valid(&c[1])) {
                return Some((p, f.to_string()));
            }
        }
    }
    // 3. source code
    let code_res = [
        // serve({ fetch: app.fetch, port: 8000 }), const port = 8000, port=8000
        Regex::new(r"(?i)\bport\b\s*[:=]\s*(?:Number\([^)]*\)\s*\|\|\s*)?(\d{2,5})\b").unwrap(),
        // process.env.PORT ?? 8000 / || "8000" / os.getenv("PORT", "8000")
        Regex::new(r#"PORT[\x22')\]]*\s*(?:\?\?|\|\||,)\s*[\x22']?(\d{2,5})"#).unwrap(),
        // app.listen(8000)
        Regex::new(r"\.listen\(\s*(\d{2,5})\b").unwrap(),
        // http.ListenAndServe(":8080"), e.Start(":8080"), r.Run(":8080")
        Regex::new(r#"(?:ListenAndServe|Start|Run|Listen)\(\s*"[^":]*:(\d{2,5})""#).unwrap(),
    ];
    let entry_names = ["index", "server", "main", "app", "serve", "dev"];
    let skip = [
        "node_modules",
        ".git",
        "dist",
        "build",
        ".next",
        "target",
        "vendor",
        "venv",
        ".venv",
        "__pycache__",
        "coverage",
        "test",
        "tests",
        "__tests__",
    ];
    let mut tiers: [HashMap<u16, (usize, String)>; 2] = [HashMap::new(), HashMap::new()];
    let walker = walkdir::WalkDir::new(root)
        .max_depth(6)
        .into_iter()
        .filter_entry(|e| !skip.iter().any(|s| e.file_name() == *s));
    for e in walker.filter_map(|e| e.ok()).take(5000) {
        let path = e.path();
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
        if !matches!(ext, "ts" | "js" | "mjs" | "cjs" | "py" | "go")
            || path.to_string_lossy().contains(".test.")
            || path.to_string_lossy().contains(".spec.")
            || e.metadata().map(|m| m.len() > 256 * 1024).unwrap_or(true)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let tier = if entry_names.contains(&stem) { 0 } else { 1 };
        for re in &code_res {
            for c in re.captures_iter(&text) {
                if let Some(p) = valid(&c[1]) {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    tiers[tier].entry(p).or_insert((0, rel)).0 += 1;
                }
            }
        }
    }
    for tier in tiers {
        if let Some((p, (_, file))) = tier
            .into_iter()
            .max_by(|a, b| a.1 .0.cmp(&b.1 .0).then(b.0.cmp(&a.0)))
        {
            return Some((p, file));
        }
    }
    None
}

fn default_sources(root: &Path, framework: Option<&str>) -> Vec<String> {
    let existing = |cands: &[&str]| -> Vec<String> {
        cands
            .iter()
            .filter(|c| root.join(c).is_dir())
            .map(|c| c.to_string())
            .collect()
    };
    let found = match framework {
        Some("nextjs") => {
            // Prefer `src/app` over `app` when both exist (Next.js ignores root `app` then).
            let mut v = Vec::new();
            if root.join("src/app").is_dir() {
                v.push("src/app".to_string());
            } else if root.join("app").is_dir() {
                v.push("app".to_string());
            }
            if root.join("src/pages").is_dir() {
                v.push("src/pages".to_string());
            } else if root.join("pages").is_dir() {
                v.push("pages".to_string());
            }
            v
        }
        Some("hono") | Some("express") => existing(&["src"]),
        Some("fastapi") => existing(&["app"])
            .into_iter()
            .chain(existing(&["src"]))
            .take(1)
            .collect(),
        _ => vec![],
    };
    if found.is_empty() {
        vec![".".to_string()]
    } else {
        found
    }
}

fn render_config(framework: Option<&str>, sources: &[String], base_url: &str) -> String {
    let mut s = String::new();
    s.push_str("# yaml-language-server: $schema=https://raw.githubusercontent.com/niti-lab/ajiro-api/main/schema/config.schema.json\n");
    s.push_str("# Ajiro API project config. Endpoint files live in api_dir (one YAML file per endpoint).\n");
    s.push_str("version: 1\n\n");
    s.push_str("# Framework scanned by `ajiro-api scan`: nextjs | hono | fastapi | express | go-nethttp | go-chi | go-echo\n");
    match framework {
        Some(f) => s.push_str(&format!("framework: {}\n\n", f)),
        None => s.push_str("# framework: hono\n\n"),
    }
    s.push_str("# Directories to scan for routes, relative to this file.\n");
    s.push_str("sources:\n");
    for d in sources {
        s.push_str(&format!("  - {}\n", yaml_scalar(d)));
    }
    s.push('\n');
    s.push_str("# Path substrings to skip while scanning (node_modules, dist, .git, ... are always skipped).\n");
    s.push_str("exclude: []\n\n");
    s.push_str("# Directory holding the endpoint YAML files.\n");
    s.push_str("api_dir: api\n\n");
    s.push_str(
        "# Dotenv file with secrets. Reference them as {{ env.NAME }} in YAML; never commit it.\n",
    );
    s.push_str("env_file: .env\n\n");
    s.push_str("# Environment used by `ajiro-api test` when --env is not given.\n");
    s.push_str("default_env: local\n\n");
    s.push_str(
        "# Environments: base URL, variables ({{ name }}) and headers sent with every request.\n",
    );
    s.push_str("environments:\n");
    s.push_str("  local:\n");
    s.push_str(&format!("    base_url: {}\n", yaml_scalar(base_url)));
    s.push_str("    # variables:\n");
    s.push_str("    #   apiVersion: v1\n");
    s.push_str("    # headers:\n");
    s.push_str("    #   Authorization: Bearer {{ env.API_TOKEN }}\n");
    s.push_str("  # staging:\n");
    s.push_str("  #   base_url: https://staging.example.com\n\n");
    s.push_str("# Extra header / JSON key names to mask in logs (password, token, authorization are masked already).\n");
    s.push_str("redact: []\n\n");
    s.push_str("# Request timeout in milliseconds.\n");
    s.push_str("timeout_ms: 10000\n");
    s
}

pub fn run_cli(dir: &Path, args: &InitArgs) -> Result<i32> {
    let root = dir;
    let config_path = root.join(CONFIG_FILE);
    if config_path.exists() && !args.force {
        bail!(
            "{} already exists. Use --force to overwrite it.",
            CONFIG_FILE
        );
    }
    if let Some(f) = &args.framework {
        if !FRAMEWORKS.contains(&f.as_str()) {
            bail!(
                "unknown framework `{}` (supported: {})",
                f,
                FRAMEWORKS.join(", ")
            );
        }
    }
    let detection = if args.framework.is_some() {
        Detection::default()
    } else {
        detect(root)
    };
    let framework = args
        .framework
        .clone()
        .or_else(|| detection.framework.clone());
    let mut warnings: Vec<String> = Vec::new();
    if framework.is_none() {
        warnings.push(no_framework_message(&detection));
    }
    let base_url = args
        .base_url
        .clone()
        .unwrap_or_else(|| default_base_url(root, framework.as_deref()));
    let sources = default_sources(root, framework.as_deref());
    let text = render_config(framework.as_deref(), &sources, &base_url);
    serde_yaml_rust2::from_str::<ProjectConfig>(&text)
        .context("internal error: generated invalid config")?;

    let mut created: Vec<String> = Vec::new();
    let mut updated: Vec<String> = Vec::new();

    std::fs::write(&config_path, &text)
        .with_context(|| format!("cannot write {}", config_path.display()))?;
    created.push(CONFIG_FILE.to_string());

    let schemas = root.join("api/schemas");
    std::fs::create_dir_all(&schemas)?;
    let keep = schemas.join(".gitkeep");
    if !keep.exists() {
        std::fs::write(&keep, "")?;
        created.push("api/schemas/.gitkeep".into());
    }
    created.push("api/".into());

    let env_example = root.join(".env.example");
    if !env_example.exists() {
        std::fs::write(
            &env_example,
            "# Secrets for ajiro-api. Copy to .env (never commit .env) and reference as {{ env.API_TOKEN }} in YAML.\nAPI_TOKEN=\n",
        )?;
        created.push(".env.example".into());
    }

    let gitignore = root.join(".gitignore");
    if gitignore.is_file() {
        let cur = std::fs::read_to_string(&gitignore)?;
        let has = cur
            .lines()
            .any(|l| matches!(l.trim(), ".env" | "/.env" | ".env*" | "*.env" | "/.env*"));
        if !has {
            let mut new = cur.clone();
            if !new.is_empty() && !new.ends_with('\n') {
                new.push('\n');
            }
            new.push_str(".env\n");
            std::fs::write(&gitignore, new)?;
            updated.push(".gitignore".into());
        }
    }

    if args.json {
        let out = serde_json::json!({
            "created": created,
            "updated": updated,
            "framework": framework,
            "warnings": warnings,
            "config": serde_json::to_value(serde_yaml_rust2::from_str::<ProjectConfig>(&text)?)?,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(crate::exit::OK);
    }

    for c in &created {
        println!("created {}", c);
    }
    for u in &updated {
        println!("updated {} (added .env)", u);
    }
    if let Some(f) = &framework {
        {
            let origin = match (&args.base_url, detect_port(root)) {
                (Some(_), _) => "from --base-url".to_string(),
                (None, Some((_, file))) => format!("port detected in {}", file),
                (None, None) => "framework default; change it in ajiro-api.yaml if needed".into(),
            };
            println!("framework: {} (base_url {}, {})", f, base_url, origin)
        }
    }
    for w in &warnings {
        println!("warning: {}", w);
    }
    println!();
    println!("Next steps:");
    println!("  1. ajiro-api scan                 # generate api/**/*.yaml from your routes");
    println!("  2. add `tests:` to the generated files (or ask your AI agent to)");
    println!("  3. ajiro-api test --env local     # run them against your running server");
    Ok(crate::exit::OK)
}
