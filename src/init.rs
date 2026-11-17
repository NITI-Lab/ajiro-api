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

/// Detect the framework used in `root` (package.json deps, pyproject/requirements, go.mod).
pub fn detect_framework(root: &Path) -> Option<String> {
    if let Some(deps) = node_deps(root) {
        let next = deps.contains("next");
        let hono = deps.contains("hono") || deps.iter().any(|d| d.starts_with("@hono/"));
        let express = deps.contains("express");
        let next_routes = ["app", "src/app"]
            .iter()
            .any(|d| has_route_handlers(&root.join(d)))
            || root.join("pages/api").is_dir()
            || root.join("src/pages/api").is_dir();
        if next && next_routes {
            return Some("nextjs".into());
        }
        if hono {
            return Some("hono".into());
        }
        if express {
            return Some("express".into());
        }
        if next {
            return Some("nextjs".into());
        }
    }
    let mut py = vec![];
    py.extend(read(root, "pyproject.toml"));
    py.extend(read(root, "Pipfile"));
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.filter_map(|e| e.ok()) {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("requirements") && n.ends_with(".txt") {
                py.extend(std::fs::read_to_string(e.path()).ok());
            }
        }
    }
    if py
        .iter()
        .any(|t| t.to_ascii_lowercase().contains("fastapi"))
    {
        return Some("fastapi".into());
    }
    if let Some(gomod) = read(root, "go.mod") {
        if gomod.contains("github.com/go-chi/chi") {
            return Some("go-chi".into());
        }
        if gomod.contains("github.com/labstack/echo") {
            return Some("go-echo".into());
        }
        return Some("go-nethttp".into());
    }
    None
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
    let framework = args.framework.clone().or_else(|| detect_framework(root));
    let base_url = args
        .base_url
        .clone()
        .unwrap_or_else(|| default_base_url(root, framework.as_deref()));
    let sources = default_sources(root, framework.as_deref());
    let text = render_config(framework.as_deref(), &sources, &base_url);
    serde_yaml::from_str::<ProjectConfig>(&text)
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
            "config": serde_json::to_value(serde_yaml::from_str::<ProjectConfig>(&text)?)?,
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
    match &framework {
        Some(f) => {
            let origin = match (&args.base_url, detect_port(root)) {
                (Some(_), _) => "from --base-url".to_string(),
                (None, Some((_, file))) => format!("port detected in {}", file),
                (None, None) => "framework default; change it in ajiro-api.yaml if needed".into(),
            };
            println!("framework: {} (base_url {}, {})", f, base_url, origin)
        }
        None => println!(
            "warning: could not detect the framework. Set `framework` in {} or run `ajiro-api scan --framework <name>`.",
            CONFIG_FILE
        ),
    }
    println!();
    println!("Next steps:");
    println!("  1. ajiro-api scan                 # generate api/**/*.yaml from your routes");
    println!("  2. add `tests:` to the generated files (or ask your AI agent to)");
    println!("  3. ajiro-api test --env local     # run them against your running server");
    Ok(crate::exit::OK)
}
