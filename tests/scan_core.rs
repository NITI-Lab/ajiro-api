//! End-to-end tests of `init` and the scan core (file creation, textual
//! `source:` updates, stale detection, `--check`).

use ajiro_api::cli::{InitArgs, ScanArgs};
use ajiro_api::config::Project;
use ajiro_api::init;
use ajiro_api::report::{ScanOptions, ScanReport, ScanStatus};
use ajiro_api::scan::{run_cli, scan_project};
use std::path::Path;

const INDEX: &str = "import { Hono } from 'hono';
const app = new Hono();

app.get('/users', listUsers);
app.post('/users', createUser);
app.get('/users/:id', getUser);

export default app;
";

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

fn init_args() -> InitArgs {
    InitArgs {
        framework: None,
        force: false,
        base_url: None,
        json: false,
    }
}

fn scan_args(check: bool) -> ScanArgs {
    ScanArgs {
        paths: vec![],
        framework: None,
        dry_run: false,
        check,
        ignore_lines: false,
        json: false,
    }
}

fn hono_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4"}}"#,
    );
    write(dir.path(), "src/index.ts", INDEX);
    assert_eq!(init::run_cli(dir.path(), &init_args()).unwrap(), 0);
    dir
}

fn scan(root: &Path, check: bool) -> ScanReport {
    let project = Project::discover(root).unwrap();
    scan_project(
        &project,
        &ScanOptions {
            check,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn first_scan_creates_files_with_expected_layout() {
    let dir = hono_project();
    let r = scan(dir.path(), false);
    assert_eq!((r.created, r.updated, r.unchanged), (3, 0, 0));
    assert_eq!(r.framework, "hono");
    let ids: Vec<_> = r.endpoints.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["users.list", "users.create", "users.get"]);
    assert_eq!(
        read(dir.path(), "api/users/post-users.yaml"),
        "# yaml-language-server: $schema=https://raw.githubusercontent.com/niti-lab/ajiro-api/main/schema/endpoint.schema.json
version: 1
id: users.create
request:
  method: POST
  path: /users
# written by ajiro-api scan \u{2014} do not edit
source:
  framework: hono
  file: src/index.ts
  line: 5
  handler: createUser
# written by humans and AI \u{2014} scan never touches this
tests: []
"
    );
    assert!(dir.path().join("api/users/get-users-id.yaml").is_file());
    // endpoint files are loadable by the rest of the tool
    let (ok, errs) = ajiro_api::project::load_endpoints(&Project::discover(dir.path()).unwrap());
    assert_eq!((ok.len(), errs.len()), (3, 0));
    // second scan: nothing to do
    let r2 = scan(dir.path(), false);
    assert_eq!((r2.created, r2.updated, r2.unchanged), (0, 0, 3));
}

#[test]
fn rescan_keeps_human_edits_byte_identical_and_updates_only_source() {
    let dir = hono_project();
    scan(dir.path(), false);
    let file = "api/users/post-users.yaml";
    let edited = "# yaml-language-server: $schema=https://example.com/s.json
# Hand written header comment
version: 1
id: users.create
request:
  method: POST
  path: /users   # trailing comment
  headers:
    Authorization: Bearer {{ auth.token }}
  body:
    name: Taro   # keep me

# scan block
source:            # scan \u{592}\u{81ea}\u{52d5}\u{3067}\u{66f4}\u{65b0}\u{3059}\u{308b}\u{6b04}
  framework: hono
  file: src/index.ts
  line: 5
  handler: createUser

# written by humans and AI
tests:
  - name: creates a user   # ok
    expect:
      status: 201
    capture:
      userId: $.id

mock:
  default: { status: 201, body: { id: usr_001 } }
";
    write(dir.path(), file, edited);
    let r = scan(dir.path(), false);
    assert_eq!((r.created, r.updated, r.unchanged), (0, 0, 3));
    assert_eq!(read(dir.path(), file), edited);

    // Move the route down by two lines: only `line:` changes.
    write(
        dir.path(),
        "src/index.ts",
        &format!("// a\n// b\n{}", INDEX),
    );
    let r = scan(dir.path(), false);
    assert_eq!((r.created, r.updated, r.unchanged), (0, 3, 0));
    assert!(r.endpoints.iter().all(|e| e.status == ScanStatus::Updated));
    assert_eq!(read(dir.path(), file), edited.replace("line: 5", "line: 7"));
    // other (untouched by humans) file got its line updated as well
    assert!(read(dir.path(), "api/users/get-users.yaml").contains("  line: 6\n"));
}

#[test]
fn missing_source_block_is_inserted_before_tests() {
    let dir = hono_project();
    let file = "api/users/get-users.yaml";
    write(
        dir.path(),
        file,
        "version: 1\nid: my.custom.id\nrequest:\n  method: GET\n  path: /users\n# humans\ntests:\n  - name: ok\n    expect:\n      status: 200\n",
    );
    let r = scan(dir.path(), false);
    let e = r
        .endpoints
        .iter()
        .find(|e| e.path == "/users" && e.method == "GET")
        .unwrap();
    assert_eq!(e.id, "my.custom.id");
    assert_eq!(e.status, ScanStatus::Updated);
    assert_eq!(
        read(dir.path(), file),
        "version: 1\nid: my.custom.id\nrequest:\n  method: GET\n  path: /users\n# written by ajiro-api scan \u{2014} do not edit\nsource:\n  framework: hono\n  file: src/index.ts\n  line: 4\n  handler: listUsers\n# humans\ntests:\n  - name: ok\n    expect:\n      status: 200\n"
    );
}

#[test]
fn removed_route_is_reported_stale_and_never_deleted() {
    let dir = hono_project();
    scan(dir.path(), false);
    write(
        dir.path(),
        "src/index.ts",
        &INDEX.replace("app.post('/users', createUser);\n", ""),
    );
    // an endpoint file without `source` is never stale
    write(
        dir.path(),
        "api/manual/get-ping.yaml",
        "version: 1\nid: manual.ping\nrequest:\n  method: GET\n  path: /ping\ntests: []\n",
    );
    let r = scan(dir.path(), false);
    assert_eq!(r.stale, vec!["api/users/post-users.yaml"]);
    assert!(dir.path().join("api/users/post-users.yaml").is_file());
    assert_eq!(r.unchanged + r.updated, 2);
}

#[test]
fn check_and_dry_run_write_nothing() {
    let dir = hono_project();
    let r = scan(dir.path(), true);
    assert_eq!(r.created, 3);
    assert!(!dir.path().join("api/users").exists());
    assert_eq!(run_cli(dir.path(), &scan_args(true)).unwrap(), 1);
    assert!(!dir.path().join("api/users").exists());
    assert_eq!(run_cli(dir.path(), &scan_args(false)).unwrap(), 0);
    assert_eq!(run_cli(dir.path(), &scan_args(true)).unwrap(), 0);
    // drift after a code change
    write(dir.path(), "src/index.ts", &format!("// c\n{}", INDEX));
    assert_eq!(run_cli(dir.path(), &scan_args(true)).unwrap(), 1);
    assert!(read(dir.path(), "api/users/post-users.yaml").contains("  line: 5\n"));
    let r = scan_project(
        &Project::discover(dir.path()).unwrap(),
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.updated, 3);
    assert!(read(dir.path(), "api/users/post-users.yaml").contains("  line: 5\n"));
}

#[test]
fn check_ignore_lines_tolerates_moved_routes_only() {
    let dir = hono_project();
    assert_eq!(run_cli(dir.path(), &scan_args(false)).unwrap(), 0);
    let ignore = ScanArgs {
        ignore_lines: true,
        ..scan_args(true)
    };
    // an edit above the routes shifts every line: reported, but not drift
    write(dir.path(), "src/index.ts", &format!("// c\n{}", INDEX));
    let r = scan(dir.path(), true);
    assert_eq!((r.updated, r.line_only), (3, 3));
    assert!(r.endpoints.iter().all(|e| e.line_only));
    assert_eq!(run_cli(dir.path(), &scan_args(true)).unwrap(), 1);
    assert_eq!(run_cli(dir.path(), &ignore).unwrap(), 0);
    // a changed handler is still drift
    write(
        dir.path(),
        "src/index.ts",
        &format!("// c\n{}", INDEX.replace("getUser", "showUser")),
    );
    let r = scan(dir.path(), true);
    assert_eq!((r.updated, r.line_only), (3, 2));
    assert_eq!(run_cli(dir.path(), &ignore).unwrap(), 1);
    // a new route is still drift
    write(
        dir.path(),
        "src/index.ts",
        &format!(
            "// c\n{}",
            INDEX.replace("export", "app.delete('/users/:id', del);\nexport")
        ),
    );
    assert_eq!(run_cli(dir.path(), &ignore).unwrap(), 1);
}

#[test]
fn colliding_ids_and_file_names_get_suffixes() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4"}}"#,
    );
    write(
        dir.path(),
        "src/index.ts",
        "import { Hono } from 'hono';\nconst app = new Hono();\napp.get('/users/:id', a);\napp.get('/users/:userId', b);\napp.get('/a/b-c', c);\napp.get('/a/b/c', d);\n",
    );
    init::run_cli(dir.path(), &init_args()).unwrap();
    let r = scan(dir.path(), false);
    assert_eq!(r.created, 4);
    let mut ids: Vec<_> = r.endpoints.iter().map(|e| e.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 4);
    assert!(ids.contains(&"users.id.get".to_string()));
    assert!(ids.contains(&"users.user-id.get".to_string()));
    let mut files: Vec<_> = r.endpoints.iter().map(|e| e.file.clone()).collect();
    files.sort();
    files.dedup();
    assert_eq!(files.len(), 4);
    assert!(files.contains(&"api/a/get-a-b-c-2.yaml".to_string()));
    let r2 = scan(dir.path(), false);
    assert_eq!((r2.created, r2.updated, r2.unchanged), (0, 0, 4));
}

fn ids_by_route(index: &str) -> Vec<(String, String)> {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4"}}"#,
    );
    write(dir.path(), "src/index.ts", index);
    init::run_cli(dir.path(), &init_args()).unwrap();
    let r = scan(dir.path(), false);
    let mut v: Vec<_> = r
        .endpoints
        .iter()
        .map(|e| (format!("{} {}", e.method, e.path), e.id.clone()))
        .collect();
    v.sort();
    v
}

#[test]
fn colliding_ids_do_not_depend_on_declaration_order() {
    let head = "import { Hono } from 'hono';\nconst app = new Hono();\n";
    let a = "app.get('/medical/offices/users', a);\napp.get('/medical/offices/:medical_office_id/users', b);\napp.get('/health', c);\napp.get('/api/health', d);\n";
    let b = "app.get('/api/health', d);\napp.get('/health', c);\napp.get('/medical/offices/:medical_office_id/users', b);\napp.get('/medical/offices/users', a);\n";
    let first = ids_by_route(&format!("{head}{a}"));
    let second = ids_by_route(&format!("{head}{b}"));
    assert_eq!(first, second);
    let get = |route: &str| {
        first
            .iter()
            .find(|(r, _)| r == route)
            .map(|(_, id)| id.as_str())
            .unwrap()
    };
    assert_eq!(
        get("GET /medical/offices/users"),
        "medical.offices.users.list"
    );
    assert_eq!(
        get("GET /medical/offices/{medical_office_id}/users"),
        "medical.offices.medical-office-id.users.list"
    );
    assert_eq!(get("GET /health"), "health.get");
    assert_eq!(get("GET /api/health"), "api.health.get");
}

#[test]
fn works_without_config_by_detecting_the_framework() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4"}}"#,
    );
    write(dir.path(), "src/index.ts", INDEX);
    assert_eq!(run_cli(dir.path(), &scan_args(false)).unwrap(), 0);
    assert!(dir.path().join("api/users/post-users.yaml").is_file());
}

#[test]
fn unknown_framework_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::discover_or_default(dir.path()).unwrap();
    assert!(scan_project(&project, &ScanOptions::default()).is_err());
    let opts = ScanOptions {
        framework: Some("rails".into()),
        ..Default::default()
    };
    assert!(scan_project(&project, &opts).is_err());
}

#[test]
fn explicit_paths_limit_scanning_and_stale() {
    let dir = hono_project();
    write(
        dir.path(),
        "src/more.ts",
        "import { Hono } from 'hono';\nconst m = new Hono();\nm.get('/more', h);\n",
    );
    scan(dir.path(), false);
    let project = Project::discover(dir.path()).unwrap();
    let opts = ScanOptions {
        paths: vec!["src/more.ts".into()],
        ..Default::default()
    };
    let r = scan_project(&project, &opts).unwrap();
    assert_eq!(r.endpoints.len(), 1);
    assert!(r.stale.is_empty());
}

#[test]
fn hundred_endpoints_scan_is_fast() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4"}}"#,
    );
    for f in 0..10 {
        let mut s = String::from("import { Hono } from 'hono';\nconst r = new Hono();\n");
        for i in 0..10 {
            s.push_str(&format!(
                "r.get('/item{}/:id', (c) => c.json({{ q: c.req.query('q') }}));\n",
                i
            ));
        }
        s.push_str("export default r;\n");
        write(dir.path(), &format!("src/routes/r{}.ts", f), &s);
    }
    let mut idx = String::from("import { Hono } from 'hono';\nconst app = new Hono();\n");
    for f in 0..10 {
        idx.push_str(&format!(
            "import r{f} from './routes/r{f}';\napp.route('/g{f}', r{f});\n"
        ));
    }
    write(dir.path(), "src/index.ts", &idx);
    init::run_cli(dir.path(), &init_args()).unwrap();
    let start = std::time::Instant::now();
    let r = scan(dir.path(), false);
    assert_eq!(r.created, 100);
    assert!(
        start.elapsed().as_secs_f64() < 3.0,
        "took {:?}",
        start.elapsed()
    );
}

// ----------------------------------------------------------------- init

#[test]
fn detects_frameworks() {
    let d = |files: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        for (n, t) in files {
            write(dir.path(), n, t);
        }
        init::detect_framework(dir.path())
    };
    assert_eq!(
        d(&[("package.json", r#"{"dependencies":{"hono":"4"}}"#)]).as_deref(),
        Some("hono")
    );
    assert_eq!(
        d(&[(
            "package.json",
            r#"{"devDependencies":{"@hono/node-server":"1"}}"#
        )])
        .as_deref(),
        Some("hono")
    );
    assert_eq!(
        d(&[("package.json", r#"{"dependencies":{"express":"4"}}"#)]).as_deref(),
        Some("express")
    );
    assert_eq!(
        d(&[("package.json", r#"{"dependencies":{"next":"15"}}"#)]).as_deref(),
        Some("nextjs")
    );
    assert_eq!(
        d(&[
            (
                "package.json",
                r#"{"dependencies":{"next":"15","express":"4"}}"#
            ),
            ("app/api/x/route.ts", "export function GET() {}")
        ])
        .as_deref(),
        Some("nextjs")
    );
    assert_eq!(
        d(&[(
            "package.json",
            r#"{"dependencies":{"next":"15","express":"4"}}"#
        )])
        .as_deref(),
        Some("express")
    );
    assert_eq!(
        d(&[("requirements.txt", "FastAPI==0.115\nuvicorn")]).as_deref(),
        Some("fastapi")
    );
    assert_eq!(
        d(&[(
            "pyproject.toml",
            "[project]\ndependencies = [\"fastapi>=0.1\"]"
        )])
        .as_deref(),
        Some("fastapi")
    );
    assert_eq!(
        d(&[(
            "go.mod",
            "module x\nrequire github.com/go-chi/chi/v5 v5.0.0"
        )])
        .as_deref(),
        Some("go-chi")
    );
    assert_eq!(
        d(&[(
            "go.mod",
            "module x\nrequire github.com/labstack/echo/v4 v4.0.0"
        )])
        .as_deref(),
        Some("go-echo")
    );
    assert_eq!(
        d(&[("go.mod", "module x\ngo 1.22")]).as_deref(),
        Some("go-nethttp")
    );
    assert_eq!(d(&[("README.md", "hi")]), None);
}

#[test]
fn init_creates_files_and_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4"}}"#,
    );
    write(dir.path(), "wrangler.toml", "name = \"x\"");
    write(dir.path(), "src/index.ts", "");
    write(dir.path(), ".gitignore", "node_modules");
    assert_eq!(init::run_cli(dir.path(), &init_args()).unwrap(), 0);
    let cfg = read(dir.path(), "ajiro-api.yaml");
    assert!(cfg.contains("framework: hono\n"));
    assert!(cfg.contains("  - src\n"));
    assert!(cfg.contains("base_url: http://localhost:8787"));
    assert!(dir.path().join("api/schemas/.gitkeep").is_file());
    assert!(dir.path().join(".env.example").is_file());
    assert!(!dir.path().join(".env").exists());
    assert_eq!(read(dir.path(), ".gitignore"), "node_modules\n.env\n");
    let project = Project::discover(dir.path()).unwrap();
    assert_eq!(project.config.framework.as_deref(), Some("hono"));

    let err = init::run_cli(dir.path(), &init_args()).unwrap_err();
    assert!(err.to_string().contains("--force"));
    let forced = InitArgs {
        framework: Some("express".into()),
        force: true,
        base_url: Some("http://localhost:4000".into()),
        json: false,
    };
    assert_eq!(init::run_cli(dir.path(), &forced).unwrap(), 0);
    let cfg = read(dir.path(), "ajiro-api.yaml");
    assert!(cfg.contains("framework: express\n") && cfg.contains("http://localhost:4000"));
    // .gitignore not appended twice
    assert_eq!(read(dir.path(), ".gitignore"), "node_modules\n.env\n");

    let bad = InitArgs {
        framework: Some("rails".into()),
        force: true,
        base_url: None,
        json: false,
    };
    assert!(init::run_cli(dir.path(), &bad).is_err());
}

#[test]
fn init_defaults_per_framework() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "requirements.txt", "fastapi\n");
    write(dir.path(), "app/main.py", "");
    init::run_cli(dir.path(), &init_args()).unwrap();
    let cfg = read(dir.path(), "ajiro-api.yaml");
    assert!(
        cfg.contains("framework: fastapi\n")
            && cfg.contains("  - app\n")
            && cfg.contains("localhost:8000")
    );

    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "go.mod",
        "module x\nrequire github.com/go-chi/chi/v5 v5.0.0",
    );
    init::run_cli(dir.path(), &init_args()).unwrap();
    let cfg = read(dir.path(), "ajiro-api.yaml");
    assert!(
        cfg.contains("framework: go-chi\n")
            && cfg.contains("  - .\n")
            && cfg.contains("localhost:8080")
    );

    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"next":"15"}}"#,
    );
    write(
        dir.path(),
        "src/app/api/x/route.ts",
        "export function GET() {}",
    );
    write(
        dir.path(),
        "src/pages/api/y.ts",
        "export default function h() {}",
    );
    init::run_cli(dir.path(), &init_args()).unwrap();
    let cfg = read(dir.path(), "ajiro-api.yaml");
    assert!(cfg.contains("  - src/app\n  - src/pages\n") && cfg.contains("localhost:3000"));
}

// ---- multi-source, basePath, .gitignore, tests, clean paths ----

fn scan_with(root: &Path, framework: &str, sources: &[&str], exclude: &[&str]) -> ScanReport {
    let config = ajiro_api::model::ProjectConfig {
        framework: Some(framework.to_string()),
        sources: sources.iter().map(|s| s.to_string()).collect(),
        exclude: exclude.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let project = Project {
        root: root.to_path_buf(),
        config,
    };
    scan_project(
        &project,
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap()
}

fn mp(r: &ScanReport) -> Vec<String> {
    r.endpoints
        .iter()
        .map(|e| {
            format!(
                "{} {} {}:{}",
                e.method, e.path, e.source.file, e.source.line
            )
        })
        .collect()
}

fn two_next_apps(dir: &Path) {
    for app in ["web", "admin"] {
        write(
            dir,
            &format!("apps/{app}/package.json"),
            r#"{"dependencies":{"next":"15"}}"#,
        );
        write(
            dir,
            &format!("apps/{app}/src/app/api/health/route.ts"),
            "export function GET() {}",
        );
    }
    write(
        dir,
        "apps/web/src/app/api/only-web/route.ts",
        "export function GET() {}",
    );
}

#[test]
fn duplicate_routes_warn_and_first_source_wins() {
    let dir = tempfile::tempdir().unwrap();
    two_next_apps(dir.path());
    for order in [
        ["apps/web/src", "apps/admin/src"],
        ["apps/admin/src", "apps/web/src"],
    ] {
        let r = scan_with(dir.path(), "nextjs", &order, &[]);
        let first = order[0].trim_end_matches("/src");
        let health = r
            .endpoints
            .iter()
            .find(|e| e.path == "/api/health")
            .unwrap();
        assert_eq!(
            health.source.file,
            format!("{first}/src/app/api/health/route.ts")
        );
        assert_eq!(r.endpoints.len(), 2, "{:?}", mp(&r));
        let dup: Vec<_> = r
            .warnings
            .iter()
            .filter(|w| w.contains("duplicate route GET /api/health"))
            .collect();
        assert_eq!(dup.len(), 1, "{:?}", r.warnings);
        assert!(
            dup[0].contains("apps/web/src/app/api/health/route.ts:1")
                && dup[0].contains("apps/admin/src/app/api/health/route.ts:1")
        );
        assert!(r.warnings.iter().any(|w| w.contains("2 Next.js apps")));
    }
}

#[test]
fn single_nextjs_app_has_no_multi_app_warning() {
    let dir = tempfile::tempdir().unwrap();
    two_next_apps(dir.path());
    let r = scan_with(dir.path(), "nextjs", &["apps/web/src"], &[]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn nextjs_base_path_literal_is_prepended_per_app() {
    let dir = tempfile::tempdir().unwrap();
    two_next_apps(dir.path());
    write(
        dir.path(),
        "apps/admin/next.config.js",
        "/** @type {import('next').NextConfig} */\nconst nextConfig = {\n  basePath: \"/admin\",\n};\nmodule.exports = nextConfig;\n",
    );
    write(
        dir.path(),
        "apps/web/next.config.mjs",
        "export default { basePath: '' };\n",
    );
    let r = scan_with(dir.path(), "nextjs", &["apps/admin/src"], &[]);
    assert_eq!(
        mp(&r),
        vec!["GET /admin/api/health apps/admin/src/app/api/health/route.ts:1"]
    );
    let r = scan_with(dir.path(), "nextjs", &["apps/web/src"], &[]);
    assert!(mp(&r)[0].starts_with("GET /api/health"), "{:?}", mp(&r));
    // both apps: the same path no longer collides
    let r = scan_with(
        dir.path(),
        "nextjs",
        &["apps/web/src", "apps/admin/src"],
        &[],
    );
    assert_eq!(r.endpoints.len(), 3);
    assert!(r.endpoints.iter().any(|e| e.path == "/admin/api/health"));
}

#[test]
fn nextjs_base_path_non_literal_warns() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"next":"15"}}"#,
    );
    write(
        dir.path(),
        "next.config.ts",
        "export default { basePath: process.env.BASE_PATH };\n",
    );
    write(
        dir.path(),
        "src/app/api/x/route.ts",
        "export function GET() {}",
    );
    let r = scan_with(dir.path(), "nextjs", &["src"], &[]);
    assert_eq!(r.endpoints[0].path, "/api/x");
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("next.config.ts") && w.contains("basePath")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn nextjs_catch_all_routes_are_summarised() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "package.json",
        r#"{"dependencies":{"next":"15"}}"#,
    );
    write(
        dir.path(),
        "src/app/api/files/[...slug]/route.ts",
        "export function GET() {}",
    );
    write(
        dir.path(),
        "src/app/api/docs/[[...slug]]/route.ts",
        "export function GET() {}",
    );
    write(
        dir.path(),
        "src/pages/api/all/[...rest].ts",
        "export default function h() {}",
    );
    let r = scan_with(dir.path(), "nextjs", &["src"], &[]);
    let w: Vec<_> = r
        .warnings
        .iter()
        .filter(|w| w.contains("catch-all"))
        .collect();
    assert_eq!(w.len(), 1, "{:?}", r.warnings);
    assert!(w[0].starts_with("3 catch-all"));
    assert!(r.endpoints.iter().any(|e| e.path == "/api/files/{slug}"));
}

#[test]
fn nextjs_route_dir_named_test_is_kept_and_ancestor_app_dir_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    // an `app` directory above the Next.js app must not shift the route root
    write(
        dir.path(),
        "app/web/package.json",
        r#"{"dependencies":{"next":"15"}}"#,
    );
    write(
        dir.path(),
        "app/web/src/app/api/test/route.ts",
        "export function GET() {}",
    );
    write(
        dir.path(),
        "app/web/src/app/api/x/route.test.ts",
        "export function GET() {}",
    );
    let r = scan_with(dir.path(), "nextjs", &["app/web/src"], &[]);
    assert_eq!(
        mp(&r),
        vec!["GET /api/test app/web/src/app/api/test/route.ts:1"]
    );
}

#[test]
fn gitignored_paths_are_skipped_including_parent_and_nested_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    write(repo, ".gitignore", "bundle/\n");
    write(repo, "pkg/sub/.gitignore", "/gen.py\n");
    let app = "from fastapi import FastAPI\napp = FastAPI()\n@app.get('/a')\ndef a(): pass\n";
    write(repo, "pkg/main.py", app);
    write(
        repo,
        "pkg/bundle/lib.py",
        &app.replace("'/a'", "'/bundled'"),
    );
    write(repo, "pkg/sub/gen.py", &app.replace("'/a'", "'/gen'"));
    write(repo, "pkg/sub/ok.py", &app.replace("'/a'", "'/ok'"));
    let r = scan_with(repo, "fastapi", &["pkg"], &[]);
    let paths: Vec<_> = r.endpoints.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["/a", "/ok"], "{:?}", r.warnings);
}

#[test]
fn test_code_is_skipped_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let app = "from fastapi import FastAPI\napp = FastAPI()\n@app.get('/a')\ndef a(): pass\n";
    write(d, "src/main.py", app);
    for f in [
        "src/tests/t.py",
        "src/test/t.py",
        "src/__tests__/t.py",
        "src/test_api.py",
        "src/api_test.py",
        "src/conftest.py",
    ] {
        write(d, f, &app.replace("'/a'", "'/leak'"));
    }
    write(d, "src/test_utils/ok.py", &app.replace("'/a'", "'/utils'"));
    let r = scan_with(d, "fastapi", &["src"], &[]);
    let paths: Vec<_> = r.endpoints.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["/a", "/utils"]);

    let go = "package main\nimport \"net/http\"\nfunc main() { http.HandleFunc(\"/a\", h) }\nfunc h(w http.ResponseWriter, r *http.Request) {}\n";
    write(d, "go/main.go", go);
    write(d, "go/main_test.go", &go.replace("\"/a\"", "\"/leak\""));
    let r = scan_with(d, "go-nethttp", &["go"], &[]);
    assert!(r.endpoints.iter().all(|e| e.path != "/leak"));
    assert!(!r.endpoints.is_empty());
}

#[test]
fn source_file_is_a_clean_path_for_sources_outside_the_project() {
    let outside = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write(outside.path(), "src/index.ts", INDEX);
    let abs = outside.path().join("src");
    let r = scan_with(project.path(), "hono", &[abs.to_str().unwrap()], &[]);
    assert_eq!(r.endpoints.len(), 3);
    for e in &r.endpoints {
        assert!(
            !e.source.file.contains("//") && e.source.file.starts_with('/'),
            "{}",
            e.source.file
        );
        assert!(!e.source.file.contains('\\'));
    }
    // go-chi used to produce `//mnt/...`
    let chi =
        std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go-chi"))
            .unwrap()
            .count();
    assert!(chi > 0);
    let go_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go-chi");
    let r = scan_with(project.path(), "go-chi", &[go_src.to_str().unwrap()], &[]);
    assert!(!r.endpoints.is_empty());
    assert!(
        r.endpoints
            .iter()
            .all(|e| !e.source.file.contains("//") && e.source.file.starts_with('/')),
        "{:?}",
        r.endpoints
            .iter()
            .map(|e| &e.source.file)
            .collect::<Vec<_>>()
    );
    // inside the project: relative
    write(project.path(), "inner/index.ts", INDEX);
    let r = scan_with(project.path(), "hono", &["inner"], &[]);
    assert_eq!(r.endpoints[0].source.file, "inner/index.ts");
}

#[test]
fn tracked_files_are_scanned_even_when_a_gitignore_pattern_matches() {
    let git = |d: &Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(d)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    if !git(d, &["init", "-q"]) {
        return; // git not installed
    }
    write(d, "package.json", r#"{"dependencies":{"next":"15"}}"#);
    write(d, "src/app/t/route.ts", "export function GET() {}");
    write(d, "src/app/u/route.ts", "export function GET() {}");
    write(d, ".gitignore", "[tu]\n");
    assert!(git(d, &["add", "-f", "src/app/t/route.ts", "package.json"]));
    let r = scan_with(d, "nextjs", &["src"], &[]);
    let paths: Vec<_> = r.endpoints.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec!["/t"]); // `/u` is ignored and untracked
}
