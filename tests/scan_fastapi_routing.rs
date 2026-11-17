//! FastAPI scanner: exact paths (trailing slashes), registration order,
//! reachability from an app, and informational warnings.
//!
//! Assertions use method + path (+ file / line / handler), never endpoint ids.

use ajiro_api::scan::{fastapi, Route};
use std::path::{Path, PathBuf};

fn collect(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect()
}

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, Vec<Route>, Vec<String>) {
    let d = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let p = d.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
    let (routes, warnings) = fastapi::scan_with_warnings(d.path(), &collect(d.path())).unwrap();
    (d, routes, warnings)
}

/// `METHOD path handler` in registration order.
fn seq(routes: &[Route]) -> Vec<String> {
    routes
        .iter()
        .map(|r| {
            format!(
                "{} {} {}",
                r.method,
                r.path,
                r.handler.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

fn sorted(routes: &[Route]) -> Vec<String> {
    let mut v: Vec<String> = routes
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    v.sort();
    v
}

fn has_warning(w: &[String], needles: &[&str]) -> bool {
    w.iter().any(|m| needles.iter().all(|n| m.contains(n)))
}

#[test]
fn trailing_slash_is_preserved_and_distinct_from_empty_path() {
    let (_d, routes, _) = project(&[(
        "main.py",
        r#"from fastapi import FastAPI, APIRouter
app = FastAPI()
r = APIRouter(prefix="/items")
@r.get("/")
def list_items(): ...
@r.get("")
def list_items2(): ...
@r.post("/")
def create_item(): ...
@app.get("/plain/")
def plain(): ...
@app.get("/")
def root(): ...
app.include_router(r)
app.include_router(r, prefix="/v2")
"#,
    )]);
    assert_eq!(
        sorted(&routes),
        vec![
            "GET /",
            "GET /items",
            "GET /items/",
            "GET /plain/",
            "GET /v2/items",
            "GET /v2/items/",
            "POST /items/",
            "POST /v2/items/",
        ]
    );
    let slash = routes.iter().find(|r| r.path == "/items/").unwrap();
    assert_eq!(slash.handler.as_deref(), Some("list_items"));
    assert_eq!(slash.line, 4);
    let plain = routes.iter().find(|r| r.path == "/items").unwrap();
    assert_eq!(plain.handler.as_deref(), Some("list_items2"));
}

#[test]
fn first_included_route_wins_and_the_shadowed_one_is_reported() {
    let (_d, routes, warnings) = project(&[
        (
            "app/main.py",
            r#"from fastapi import FastAPI
from app.routers import a_first, z_last
app = FastAPI()
app.include_router(z_last.router)
app.include_router(a_first.router)
"#,
        ),
        ("app/__init__.py", ""),
        ("app/routers/__init__.py", ""),
        (
            "app/routers/z_last.py",
            r#"from fastapi import APIRouter
router = APIRouter(prefix="/api/usage")
@router.get("")
def from_z(): ...
"#,
        ),
        (
            "app/routers/a_first.py",
            r#"from fastapi import APIRouter
router = APIRouter()
@router.get("/api/usage")
def from_a(): ...
"#,
        ),
    ]);
    assert_eq!(
        seq(&routes),
        vec!["GET /api/usage from_z", "GET /api/usage from_a"]
    );
    assert!(has_warning(
        &warnings,
        &["duplicate", "GET /api/usage", "a_first.py:3", "z_last.py:3"]
    ));
}

#[test]
fn routes_not_reachable_from_an_app_are_skipped_with_one_warning() {
    let (_d, routes, warnings) = project(&[
        (
            "main.py",
            r#"from fastapi import FastAPI
import dead, live
app = FastAPI()
app.include_router(live.router)
"#,
        ),
        (
            "live.py",
            "from fastapi import APIRouter\nrouter = APIRouter(prefix=\"/live\")\n@router.get(\"/x\")\ndef x(): ...\n",
        ),
        (
            "dead.py",
            "from fastapi import APIRouter\nrouter = APIRouter(prefix=\"/dead\")\n@router.get(\"/x\")\ndef x(): ...\n",
        ),
        (
            "tests/test_probe.py",
            r#"from fastapi import FastAPI, APIRouter
def test_probe():
    app = FastAPI()

    @app.get("/rate-probe")
    async def probe(): ...

def test_insert():
    router = APIRouter()

    @router.get("/inserted")
    async def inserted(): ...
"#,
        ),
    ]);
    assert_eq!(sorted(&routes), vec!["GET /live/x"]);
    let skipped: Vec<_> = warnings.iter().filter(|w| w.contains("skipped")).collect();
    assert_eq!(skipped.len(), 1, "{warnings:?}");
    assert!(skipped[0].contains("dead.py") && skipped[0].contains("test_probe.py"));
}

#[test]
fn library_project_without_an_app_still_emits_routes_and_warns() {
    let (_d, routes, warnings) = project(&[(
        "lib.py",
        "from fastapi import APIRouter\nrouter = APIRouter(prefix=\"/lib\")\n@router.get(\"/a\")\ndef a(): ...\n",
    )]);
    assert_eq!(sorted(&routes), vec!["GET /lib/a"]);
    assert!(has_warning(&warnings, &["no module-level FastAPI"]));
}

#[test]
fn include_chains_loops_factories_and_setup_functions_are_followed() {
    let (_d, routes, warnings) = project(&[
        (
            "main.py",
            r#"from fastapi import FastAPI, APIRouter
import a, b, c, d

def create_app(public_only: bool = False):
    api = FastAPI()
    for router in (() if public_only else (a.router, b.router)):
        api.include_router(router)

    @api.get("/healthz")
    def healthz(): ...

    register(api)
    return api

def register(app):
    app.include_router(c.router, prefix="/c")

parent = APIRouter(prefix="/p")
parent.include_router(d.router, prefix="/d")
ROUTERS = [parent]
app = create_app()
for r in ROUTERS:
    app.include_router(r)
"#,
        ),
        (
            "a.py",
            "from fastapi import APIRouter\nrouter = APIRouter(prefix=\"/a\")\n@router.get(\"/\")\ndef a(): ...\n",
        ),
        (
            "b.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.api_route(\"/b\", methods=[\"GET\", \"POST\"])\n@router.get(\"/b2\")\ndef b(): ...\n",
        ),
        (
            "c.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get(\"/c\")\ndef c(): ...\n",
        ),
        (
            "d.py",
            "from fastapi import APIRouter\nrouter = APIRouter()\n@router.get(\"/\")\ndef d(): ...\n",
        ),
    ]);
    assert_eq!(
        sorted(&routes),
        vec![
            "GET /a/",
            "GET /b",
            "GET /b2",
            "GET /c/c",
            "GET /healthz",
            "GET /p/d/",
            "POST /b",
        ]
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn path_converter_websockets_and_mounts_are_reported() {
    let (_d, routes, warnings) = project(&[(
        "main.py",
        r#"from fastapi import FastAPI
from fastapi.staticfiles import StaticFiles
app = FastAPI()
sub = FastAPI()

@sub.get("/ping")
def ping(): ...

@app.get("/files/{filename:path}")
def get_file(filename: str): ...

@app.websocket("/ws/{room}")
async def ws(room: str): ...

app.mount("/static", StaticFiles(directory="s"))
app.mount("/sub", sub)
"#,
    )]);
    assert_eq!(
        sorted(&routes),
        vec!["GET /files/{filename}", "GET /sub/ping"]
    );
    assert!(has_warning(
        &warnings,
        &["{name:path}", "GET /files/{filename}", "main.py:9"]
    ));
    assert!(has_warning(
        &warnings,
        &[
            "not scanned",
            "websocket",
            "/ws/{room}",
            "mount",
            "/static",
            "main.py"
        ]
    ));
    assert!(!has_warning(&warnings, &["/sub (", "mount(s): /sub"]));
}

#[test]
fn rescan_keeps_trailing_slash_endpoints_matched() {
    use ajiro_api::config::Project;
    use ajiro_api::report::ScanOptions;
    use ajiro_api::scan::scan_project;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("ajiro-api.yaml"),
        "version: 1\nframework: fastapi\nsources: [.]\napi_dir: api\n",
    )
    .unwrap();
    std::fs::write(
        root.join("main.py"),
        "from fastapi import FastAPI, APIRouter\napp = FastAPI()\nr = APIRouter(prefix=\"/items\")\n@r.get(\"/\")\ndef a(): ...\n@r.get(\"\")\ndef b(): ...\n@app.get(\"/plain/\")\ndef c(): ...\napp.include_router(r)\n",
    )
    .unwrap();
    let project = Project::discover(root).unwrap();
    let first = scan_project(&project, &ScanOptions::default()).unwrap();
    assert_eq!(first.created, 3, "{:?}", first.endpoints);
    let second = scan_project(&project, &ScanOptions::default()).unwrap();
    assert_eq!(
        (second.created, second.unchanged, second.stale.len()),
        (0, 3, 0),
        "{:?}",
        second
    );
}
