//! Integration tests for the FastAPI and Go (net/http, chi, Echo) scanners.
//!
//! Expected entries are `METHOD path file:line handler query_params framework`
//! (`-` = none), compared as sorted lists.

use ajiro_api::scan::{fastapi, go, Route};
use std::path::{Path, PathBuf};

fn collect(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn fmt(routes: &[Route]) -> Vec<String> {
    let mut v: Vec<String> = routes
        .iter()
        .map(|r| {
            let q = if r.query_params.is_empty() {
                "-".to_string()
            } else {
                r.query_params.join(",")
            };
            format!(
                "{} {} {}:{} {} {} {}",
                r.method,
                r.path,
                r.file,
                r.line,
                r.handler.as_deref().unwrap_or("-"),
                q,
                r.framework
            )
        })
        .collect();
    v.sort();
    v
}

fn check(expected: &[&str], actual: &[Route]) {
    let mut e: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    e.sort();
    assert_eq!(fmt(actual), e);
}

fn scan_py(root: &Path) -> Vec<Route> {
    fastapi::scan(root, &collect(root)).unwrap()
}

fn scan_go(root: &Path) -> Vec<Route> {
    go::scan(root, &collect(root)).unwrap()
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

#[test]
fn fastapi_project() {
    let root = fixture("fastapi");
    check(
        &[
        "GET /health app/main.py:15 health - fastapi",
        "GET / app/main.py:20 root - fastapi",
        "GET /echo app/main.py:25 echo msg fastapi",
        "POST /echo app/main.py:25 echo msg fastapi",
        "GET /ping app/main.py:30 health - fastapi",
        "OPTIONS /admin/stats app/routers/admin/stats.py:6 stats_options window fastapi",
        "GET /admin/stats/{name} app/routers/admin/stats.py:11 stats since,flag fastapi",
        "GET /api/items/ app/routers/items.py:9 list_items page,size fastapi",
        "GET /api/items/v2/{item_id}/reviews/ app/routers/items.py:14 list_reviews stars fastapi",
        "POST /api/items/v2/{item_id}/reviews/ app/routers/items.py:19 add_review - fastapi",
        "GET /api/v1/users/ app/routers/users.py:11 list_users limit,offset,role,tags fastapi",
        "POST /api/v1/users app/routers/users.py:24 create_user dry_run fastapi",
        "GET /api/v1/users/{user_id} app/routers/users.py:29 get_user verbose,q fastapi",
        "PUT /api/v1/users/{user_id} app/routers/users.py:34 replace_user - fastapi",
        "DELETE /api/v1/users/{user_id} app/routers/users.py:39 delete_user force fastapi",
        ],
        &scan_py(&root),
    );
}

#[test]
fn fastapi_router_never_included_is_skipped() {
    let root = fixture("fastapi");
    let (routes, warnings) = fastapi::scan_with_warnings(&root, &collect(&root)).unwrap();
    assert!(routes.iter().all(|r| !r.path.starts_with("/orphans")));
    assert!(
        warnings.iter().any(|w| w.contains("orphan.py")),
        "{warnings:?}"
    );
}

#[test]
fn go_nethttp_project() {
    let root = fixture("go-nethttp");
    check(
        &[
            "GET /admin/stats admin.go:6 adminStats - go-nethttp",
            "PUT /admin/settings/{key} admin.go:7 adminSetting - go-nethttp",
            "GET /users/{id} main.go:13 getUser - go-nethttp",
            "POST /users main.go:14 createUser - go-nethttp",
            "DELETE /users/{id} main.go:15 deleteUser - go-nethttp",
            "GET /files/{path} main.go:16 serveFile - go-nethttp",
            "GET / main.go:17 index - go-nethttp",
            "POST /webhooks main.go:18 webhook - go-nethttp",
            "GET /legacy main.go:19 legacy - go-nethttp",
            "GET /healthz main.go:20 health - go-nethttp",
            "GET /static main.go:21 - - go-nethttp",
            "GET /metrics main.go:22 metrics - go-nethttp",
            "GET /closure main.go:23 - - go-nethttp",
            "GET /api/items main.go:26 listItems - go-nethttp",
            "GET /api/items/{itemID} main.go:27 getItem - go-nethttp",
            "GET /ping main.go:32 ping - go-nethttp",
        ],
        &scan_go(&root),
    );
}

#[test]
fn go_chi_project() {
    let root = fixture("go-chi");
    check(
        &[
            "GET /admin/stats admin.go:7 adminStats - go-chi",
            "DELETE /admin/users/{userID} admin.go:9 adminDeleteUser - go-chi",
            "GET /admin/audit admin.go:17 auditLog - go-chi",
            "GET / main.go:14 home - go-chi",
            "GET /health main.go:15 health - go-chi",
            "GET /version main.go:16 version - go-chi",
            "POST /echo main.go:17 echo - go-chi",
            "GET /users main.go:20 listUsers - go-chi",
            "POST /users main.go:21 createUser - go-chi",
            "GET /users/{id} main.go:23 getUser - go-chi",
            "PUT /users/{id} main.go:24 updateUser - go-chi",
            "DELETE /users/{id} main.go:25 deleteUser - go-chi",
            "GET /users/{id}/posts main.go:26 listUserPosts - go-chi",
            "GET /me main.go:32 me - go-chi",
            "POST /me/avatar main.go:33 uploadAvatar - go-chi",
            "GET /reports/daily main.go:39 dailyReport - go-chi",
            "PATCH /reports/{id} main.go:40 patchReport - go-chi",
            "GET /assets/{wildcard} main.go:43 - - go-chi",
        ],
        &scan_go(&root),
    );
}

#[test]
fn go_echo_project() {
    let root = fixture("go-echo");
    check(
        &[
            "GET / main.go:18 index - go-echo",
            "GET /health main.go:19 - - go-echo",
            "GET /any main.go:20 anyHandler - go-echo",
            "GET /match main.go:21 matchHandler - go-echo",
            "POST /match main.go:21 matchHandler - go-echo",
            "PATCH /added main.go:22 addedHandler - go-echo",
            "GET /api/ping main.go:25 ping - go-echo",
            "GET /api/v1/status main.go:28 status - go-echo",
            "GET /api/v1/users users.go:7 listUsers - go-echo",
            "POST /api/v1/users users.go:8 createUser - go-echo",
            "GET /api/v1/users/{id} users.go:9 getUser - go-echo",
            "DELETE /api/v1/users/{id} users.go:10 deleteUser - go-echo",
            "GET /orders users.go:16 h.List - go-echo",
            "GET /orders/{orderID} users.go:17 h.Get - go-echo",
            "POST /orders/{orderID}/cancel users.go:18 h.Cancel - go-echo",
        ],
        &scan_go(&root),
    );
}

#[test]
fn fastapi_module_import_styles() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "src/pkg/__init__.py", "");
    write(
        r,
        "src/pkg/api/__init__.py",
        "from .v1 import router as v1_router\n",
    );
    write(
        r,
        "src/pkg/api/v1.py",
        "from fastapi import APIRouter\nrouter = APIRouter(prefix='/v1')\n\n@router.get('/a')\ndef a(x: int): ...\n",
    );
    write(
        r,
        "src/pkg/main.py",
        "import pkg.api.v1 as v1mod\nfrom pkg.api import v1_router\nfrom fastapi import FastAPI as F, APIRouter as R\n\
         app = F()\nouter = R(prefix='/outer')\nouter.include_router(v1_router, prefix='/in')\n\
         app.include_router(outer, prefix='/top')\napp.include_router(v1mod.router, prefix='/direct')\n\
         @app.post('/x')\nasync def x(): ...\n",
    );
    let routes = scan_py(r);
    check(
        &[
            "GET /direct/v1/a src/pkg/api/v1.py:4 a x fastapi",
            "GET /top/outer/in/v1/a src/pkg/api/v1.py:4 a x fastapi",
            "POST /x src/pkg/main.py:9 x - fastapi",
        ],
        &routes,
    );
}

#[test]
fn fastapi_dynamic_paths_skipped_and_garbage_ok() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "a.py",
        "from fastapi import FastAPI\napp = FastAPI()\n@app.get(f'/{x}')\ndef a(): ...\n@app.get(PATH)\ndef b(): ...\n@app.get('/ok')\ndef c(): ...\n",
    );
    write(r, "broken.py", "def (((:\n@@@ class\n");
    write(r, "weird.go", "package \x00\x01 func {{{");
    write(r, "binary.py", "\u{0}\u{1}\u{2}");
    check(&["GET /ok a.py:7 c - fastapi"], &scan_py(r));
    assert!(scan_go(r).is_empty());
}

#[test]
fn go_package_const_and_helper_prefix() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "main.go",
        "package main\nimport \"github.com/labstack/echo/v4\"\nconst base = \"/api\"\nfunc main() {\n\te := echo.New()\n\tg := e.Group(base + \"/v2\")\n\tmount(g)\n\tmount(e.Group(dyn()))\n}\nfunc mount(g *echo.Group) {\n\tg.GET(\"/z\", z)\n}\n",
    );
    check(&["GET /api/v2/z main.go:11 z - go-echo"], &scan_go(r));
}

#[test]
fn scales_to_hundreds_of_endpoints() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let mut py = String::from("from fastapi import FastAPI\napp = FastAPI()\n");
    let mut go = String::from("package main\nimport \"github.com/go-chi/chi/v5\"\nfunc main() {\n\tr := chi.NewRouter()\n");
    for i in 0..300 {
        py.push_str(&format!(
            "@app.get('/r{i}/{{id}}')\ndef f{i}(id: int, q: str = ''): ...\n"
        ));
        go.push_str(&format!("\tr.Get(\"/r{i}/{{id}}\", h{i})\n"));
    }
    go.push_str("}\n");
    write(r, "app.py", &py);
    write(r, "main.go", &go);
    let t = std::time::Instant::now();
    let p = scan_py(r);
    let g = scan_go(r);
    assert_eq!(p.len(), 300);
    assert_eq!(g.len(), 300);
    assert!(
        t.elapsed().as_secs_f64() < 1.0,
        "too slow: {:?}",
        t.elapsed()
    );
}
