//! Route extraction for the TypeScript frameworks against fixture projects.

use ajiro_api::config::Project;
use ajiro_api::model::ProjectConfig;
use ajiro_api::report::{ScanOptions, ScanReport};
use ajiro_api::scan::scan_project;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn project(root: &Path, fw: &str) -> Project {
    let config = ProjectConfig {
        framework: Some(fw.to_string()),
        ..Default::default()
    };
    Project {
        root: root.to_path_buf(),
        config,
    }
}

fn scan(fw: &str) -> ScanReport {
    scan_project(
        &project(&fixture(fw), fw),
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap()
}

fn lines(r: &ScanReport) -> Vec<String> {
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

fn copy_files(fw: &str, files: &[&str], to: &Path) {
    for f in files {
        let dest = to.join(f);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(fixture(fw).join(f), dest).unwrap();
    }
}

#[test]
fn nextjs_routes() {
    let r = scan("nextjs");
    assert_eq!(
        lines(&r),
        vec![
            "GET /api/files/{path} app/api/files/[[...path]]/route.ts:1",
            "HEAD /api/files/{path} app/api/files/[[...path]]/route.ts:2",
            "GET /api/legacy pages/api/legacy.ts:1",
            "POST /api/legacy pages/api/legacy.ts:1",
            "DELETE /api/orders/{id} pages/api/orders/[id].ts:3",
            "GET /api/orders/{id} pages/api/orders/[id].ts:3",
            "GET /api/ping pages/api/ping/index.ts:2",
            "GET /api/posts/{slug} app/api/posts/[...slug]/route.js:5",
            "POST /api/posts/{slug} app/api/posts/[...slug]/route.js:5",
            "GET /api/search app/api/search/route.ts:3",
            "GET /api/users app/api/users/route.ts:3",
            "POST /api/users app/api/users/route.ts:7",
            "DELETE /api/users/{id} app/api/users/[id]/route.ts:9",
            "GET /api/users/{id} app/api/users/[id]/route.ts:5",
            "PUT /api/users/{id} app/api/users/[id]/route.ts:7",
            "GET /health app/(marketing)/health/route.ts:1",
            "GET /login app/@modal/login/route.ts:1",
        ]
    );
    assert!(r.endpoints.iter().all(|e| e.source.framework == "nextjs"));
    let handlers: Vec<_> = r
        .endpoints
        .iter()
        .filter(|e| e.path == "/api/posts/{slug}" || e.path == "/api/orders/{id}")
        .map(|e| e.source.handler.clone().unwrap())
        .collect();
    assert_eq!(
        handlers,
        vec!["orderHandler", "orderHandler", "handler", "handler"]
    );
}

#[test]
fn nextjs_skips_node_modules_and_writes_query_params() {
    let dir = tempfile::tempdir().unwrap();
    copy_files("nextjs", &["app/api/search/route.ts"], dir.path());
    let nm = dir.path().join("node_modules/x/app");
    std::fs::create_dir_all(&nm).unwrap();
    std::fs::write(
        nm.join("route.ts"),
        "export function GET() { return new Response('x'); }",
    )
    .unwrap();
    let r = scan_project(&project(dir.path(), "nextjs"), &ScanOptions::default()).unwrap();
    assert_eq!(r.created, 1);
    let text = std::fs::read_to_string(dir.path().join("api/search/get-api-search.yaml")).unwrap();
    assert!(
        text.contains("  query:\n    q: null\n    page: null\n"),
        "{}",
        text
    );
}

#[test]
fn hono_routes_with_cross_file_mounts() {
    let r = scan("hono");
    assert_eq!(
        lines(&r),
        vec![
            "GET /any src/index.ts:11",
            "GET /api/version src/index.ts:20",
            "GET /health src/index.ts:10",
            "GET /internal/admin/stats src/routes/admin.ts:5",
            "GET /items src/routes/items.ts:12",
            "GET /items/{id} src/routes/items.ts:11",
            "GET /posts src/routes/posts/index.ts:5",
            "POST /posts src/routes/posts/index.ts:5",
            "DELETE /posts/{postId} src/routes/posts/index.ts:6",
            "GET /posts/{postId}/comments src/routes/posts/index.ts:7",
            "GET /users src/routes/users.ts:6",
            "POST /users src/routes/users.ts:7",
            "GET /users/search src/routes/users.ts:13",
            "DELETE /users/{id} src/routes/users.ts:11",
            "GET /users/{id} src/routes/users.ts:9",
            "PUT /users/{id} src/routes/users.ts:10",
        ]
    );
    assert!(r
        .warnings
        .iter()
        .any(|w| w.contains("src/index.ts:11") && w.contains(".all()")));
    assert!(r
        .warnings
        .iter()
        .any(|w| w.contains("src/index.ts:12") && w.contains("not statically known")));
    let handler = |p: &str, m: &str| {
        r.endpoints
            .iter()
            .find(|e| e.path == p && e.method == m)
            .unwrap()
            .source
            .handler
            .clone()
    };
    assert_eq!(handler("/users", "GET").as_deref(), Some("listUsers"));
    assert_eq!(handler("/users", "POST").as_deref(), Some("createUser"));
    assert_eq!(handler("/health", "GET"), None);
}

#[test]
fn hono_basepath_and_query_params_from_inline_handlers() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("index.ts"),
        "import { Hono } from 'hono';\nconst app = new Hono().basePath('/v1');\napp.get('/s', (c) => c.json([c.req.query('name'), c.req.query('limit')]));\nexport default app;\n",
    )
    .unwrap();
    let r = scan_project(&project(dir.path(), "hono"), &ScanOptions::default()).unwrap();
    assert_eq!(lines(&r), vec!["GET /v1/s src/index.ts:3"]);
    let text = std::fs::read_to_string(dir.path().join(&r.endpoints[0].file)).unwrap();
    assert!(
        text.contains("    name: null\n    limit: null\n"),
        "{}",
        text
    );
}

#[test]
fn hono_basepath_derived_instance() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.ts"),
        "import { Hono } from 'hono';\nconst app = new Hono();\nconst v1 = app.basePath('/v1');\nv1.post('/login', login);\napp.get('/', home);\nexport default app;\n",
    )
    .unwrap();
    let r = scan_project(
        &project(dir.path(), "hono"),
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(lines(&r), vec!["GET / app.ts:5", "POST /v1/login app.ts:4"]);
}

#[test]
fn express_routes_with_commonjs_and_esm_mounts() {
    let r = scan("express");
    assert_eq!(
        lines(&r),
        vec![
            "GET /api/orders src/routes/orders.ts:6",
            "DELETE /api/orders/{orderId} src/routes/orders.ts:8",
            "GET /api/orders/{orderId} src/routes/orders.ts:7",
            "GET /api/orders/{orderId}/items src/routes/order-items.ts:5",
            "POST /api/orders/{orderId}/items src/routes/order-items.ts:6",
            "GET /health src/app.js:8",
            "GET /items src/app.js:15",
            "POST /items src/app.js:16",
            "GET /users src/routes/users.js:4",
            "POST /users src/routes/users.js:9",
            "GET /users/{id} src/routes/users.js:8",
        ]
    );
    assert!(r.warnings.iter().any(|w| w.contains("src/app.js:18")));
    let items = r
        .endpoints
        .iter()
        .find(|e| e.path == "/items" && e.method == "GET")
        .unwrap();
    assert_eq!(items.source.handler.as_deref(), Some("ctrl.list"));
}

#[test]
fn express_query_destructuring_and_node_modules() {
    let dir = tempfile::tempdir().unwrap();
    copy_files(
        "express",
        &[
            "src/app.js",
            "src/routes/users.js",
            "src/routes/orders.ts",
            "src/routes/order-items.ts",
            "src/controllers/items.js",
        ],
        dir.path(),
    );
    let nm = dir.path().join("node_modules/express");
    std::fs::create_dir_all(&nm).unwrap();
    std::fs::write(
        nm.join("index.js"),
        "const express = require('express'); const a = express(); a.get('/nope', () => 1);",
    )
    .unwrap();
    let r = scan_project(&project(dir.path(), "express"), &ScanOptions::default()).unwrap();
    assert!(
        !lines(&r).iter().any(|l| l.contains("/nope")),
        "node_modules must be skipped"
    );
    let text = std::fs::read_to_string(dir.path().join("api/users/get-users.yaml")).unwrap();
    assert!(
        text.contains("    page: null\n    limit: null\n"),
        "{}",
        text
    );
}

#[test]
fn hono_openapi_factories_createroute_and_openapi_chains() {
    let r = scan_project(
        &project(&fixture("hono-openapi"), "hono"),
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        lines(&r),
        vec![
            "GET /api src/routes/root/root.index.ts:6",
            "GET /api/admin/grades src/router/admin.ts:31",
            "GET /api/admin/ping src/router/admin.ts:32",
            "GET /api/admin/stats src/router/admin.ts:15",
            "GET /api/admin/students src/router/admin.ts:27",
            "POST /api/admin/students src/router/admin.ts:29",
            "GET /api/admin/students/export src/router/admin.ts:33",
            "GET /api/admin/students/{studentId} src/router/admin.ts:28",
            "PATCH /api/admin/students/{studentId} src/router/admin.ts:30",
            "GET /api/health src/router/index.ts:5",
            "GET /api/users src/routes/users/users.index.ts:7",
            "POST /api/users src/routes/users/users.index.ts:8",
            "DELETE /api/users/{id} src/routes/users/users.index.ts:10",
            "GET /api/users/{id} src/routes/users/users.index.ts:9",
            "POST /api/v1/reports src/routes/reports/reports.index.ts:10",
            "GET /api/v1/reports/{id} src/routes/reports/reports.index.ts:9",
            "GET /api/v1/tasks src/routes/tasks/tasks.index.ts:7",
            "PATCH /api/v1/tasks/{id} src/routes/tasks/tasks.index.ts:8",
            "GET /doc src/lib/configure-open-api.ts:6",
            "GET /legacy/ping src/routes/legacy.routes.ts:4",
            "GET /reference src/lib/configure-open-api.ts:11",
        ]
    );
    assert!(r.endpoints.iter().all(|e| e.source.framework == "hono"));
    // Object-literal route groups resolve (no unresolved warning, no orphans
    // without the mount prefix); `.on()` middleware and `.use()` emit nothing.
    assert!(
        !r.warnings
            .iter()
            .any(|w| w.contains("could not be resolved")),
        "{:?}",
        r.warnings
    );
    assert!(
        !r.endpoints
            .iter()
            .any(|e| e.path.starts_with("/students") || e.path.contains("{}")),
        "{:?}",
        lines(&r)
    );
    // The orphan createRoute() is reported, not silently emitted.
    assert!(
        r.warnings
            .iter()
            .any(|w| w.starts_with("1 createRoute() definition(s)")
                && w.contains("no mount prefixes")),
        "{:?}",
        r.warnings
    );
    // A template literal with a non-const substitution is skipped loudly.
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("reports.routes.ts:") && w.contains("not statically known")),
        "{:?}",
        r.warnings
    );
    let find = |p: &str, m: &str| {
        r.endpoints
            .iter()
            .find(|e| e.path == p && e.method == m)
            .unwrap()
    };
    assert_eq!(
        find("/api/users", "GET").source.handler.as_deref(),
        Some("handlers.list")
    );
}

#[test]
fn hono_unconnected_createroute_is_emitted_with_warning() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.routes.ts"),
        "import { createRoute } from '@hono/zod-openapi';\nexport const a = createRoute({ method: 'get', path: '/a/{id}' });\nexport const b = createRoute({ method: 'post', path: `/b` });\n",
    )
    .unwrap();
    let r = scan_project(
        &project(dir.path(), "hono"),
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        lines(&r),
        vec!["GET /a/{id} a.routes.ts:2", "POST /b a.routes.ts:3"]
    );
    assert!(r
        .warnings
        .iter()
        .any(|w| w.starts_with("2 createRoute() definition(s)")));
}

#[test]
fn hono_zero_routes_hints_at_openapi_import() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        "{\"dependencies\":{\"@hono/zod-openapi\":\"0.18.0\"}}",
    )
    .unwrap();
    std::fs::write(dir.path().join("a.ts"), "export const x = 1;\n").unwrap();
    let r = scan_project(
        &project(dir.path(), "hono"),
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r.endpoints.is_empty());
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("no hono routes found") && w.contains("ajiro-api openapi import")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn hono_factory_variants_and_mount_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let w = |name: &str, text: &str| {
        let p = dir.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    // Arrow factory with a return type annotation, behind a transitive wrapper.
    w(
        "src/lib/factory.ts",
        "import { Hono } from 'hono';\nexport const base = (): Hono => buildIt();\nexport const make = () => base();\n",
    );
    // Anonymous default export factory.
    w(
        "src/lib/anon.ts",
        "import { OpenAPIHono } from '@hono/zod-openapi';\nexport default () => new OpenAPIHono();\n",
    );
    w("src/lib/consts.ts", "export const PREFIX = '/pre';\n");
    w(
        "src/routers/a.ts",
        "import make from '../lib/anon';\nimport { PREFIX } from '../lib/consts';\nconst a = make();\na.get(`${PREFIX}/a`, (c) => c.text('a'));\nexport default a;\n",
    );
    w(
        "src/routers/b.ts",
        "import { make } from '../lib/factory';\nexport const b = make();\nb.post('/b', (c) => c.text('b'));\n",
    );
    w(
        "src/routers/index.ts",
        "export { default as a } from './a';\nexport * from './b';\n",
    );
    w(
        "src/app.ts",
        "import { make } from './lib/factory';\nimport * as r from './routers';\nimport { a, b } from './routers';\nconst app = make().basePath('/x');\nconst list = [a, b];\nfor (const router of list) {\n  app.route('/loop', router);\n}\napp.route('/ns', r.a);\nexport default app;\n",
    );
    let r = scan_project(
        &project(dir.path(), "hono"),
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        lines(&r),
        vec![
            "POST /x/loop/b src/routers/b.ts:3",
            "GET /x/loop/pre/a src/routers/a.ts:4",
            "GET /x/ns/pre/a src/routers/a.ts:4",
        ],
        "{:?}",
        r.warnings
    );
}

fn scan_sources(fw: &str, files: &[(&str, &str)]) -> ScanReport {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let dest = dir.path().join(name);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(dest, text).unwrap();
    }
    scan_project(
        &project(dir.path(), fw),
        &ScanOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap()
}

fn method_paths(r: &ScanReport) -> Vec<String> {
    r.endpoints
        .iter()
        .map(|e| format!("{} {}", e.method, e.path))
        .collect()
}

#[test]
fn hono_on_with_only_call_handlers_is_middleware_when_paths_are_registered_elsewhere() {
    let r = scan_sources(
        "hono",
        &[(
            "app.ts",
            r#"import { Hono } from "hono";
const app = new Hono();
app.on(["POST", "PATCH"], ["/b", "/b/:id"], auth("X"));
app.post("/b", create);
app.patch("/b/:bid", update);
app.get("/reference", apiReference({}));
app.get("/open", zValidator("json", s));
app.get("/x", auth("a"));
app.get("/x", (c) => c.json({}));
app.on(["GET"], ["/b", "/nowhere"], auth("Z"));
app.use("/b/*", auth("Y"));
"#,
        )],
    );
    let mut got = method_paths(&r);
    got.sort();
    assert_eq!(
        got,
        vec![
            "GET /b",
            "GET /nowhere",
            "GET /open",
            "GET /reference",
            "GET /x",
            "PATCH /b/{bid}",
            "POST /b",
        ],
        "{:?}",
        r.warnings
    );
}

#[test]
fn express_call_only_registration_is_middleware_when_path_is_registered_elsewhere() {
    let r = scan_sources(
        "express",
        &[(
            "app.js",
            r#"const express = require("express");
const app = express();
app.get("/x", auth());
app.get("/x", ctrl.list);
app.get("/only-mw", auth());
"#,
        )],
    );
    let mut got = method_paths(&r);
    got.sort();
    assert_eq!(got, vec!["GET /only-mw", "GET /x"]);
}

#[test]
fn hono_unnamed_path_params_become_param_with_warning() {
    let r = scan_sources(
        "hono",
        &[(
            "app.ts",
            r#"import { Hono } from "hono";
const app = new Hono();
const NAME = ":bannerId";
app.on(["POST"], ["/banners/:", `/by-const/${NAME}`, `/unknown/${process.env.X}`], h);
app.get("/f/:{[0-9]+}", h);
app.get("/g/:id{[^/]+}/x", h);
app.get("/h/:id?", h);
app.get("/i/:id{[0-9]+}", h);
"#,
        )],
    );
    let mut got = method_paths(&r);
    got.sort();
    assert_eq!(
        got,
        vec![
            "GET /f/{param}",
            "GET /g/{id}/x",
            "GET /h/{id}",
            "GET /i/{id}",
            "POST /banners/{param}",
            "POST /by-const/{bannerId}",
        ],
        "{:?}",
        r.warnings
    );
    assert!(r.endpoints.iter().all(|e| !e.path.contains("{}")));
    for raw in ["raw path: `/banners/:`", "raw path: `/f/:{[0-9]+}`"] {
        assert!(
            r.warnings.iter().any(|w| w.contains(raw)),
            "{} in {:?}",
            raw,
            r.warnings
        );
    }
    // The template with an unknown substitution inside the array is reported.
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("some paths in the array are not statically known")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn hono_route_objects_default_import_aliases_and_nested_props() {
    let r = scan_sources(
        "hono",
        &[
            (
                "routes.ts",
                r#"import { createRoute } from "@hono/zod-openapi";
const a = createRoute({ method: "get", path: "/a" });
const b = createRoute({ method: "get", path: "/b" });
const group = { a, nested: { b, c: createRoute({ method: "post", path: "/c" }) } } as const;
export default group;
export const alias = group.nested;
"#,
            ),
            (
                "app.ts",
                r#"import { OpenAPIHono } from "@hono/zod-openapi";
import group, { alias } from "./routes";
import * as ns from "./routes";
const api = new OpenAPIHono()
  .openapi(group.a, h)
  .openapi(group["nested"]["b"], h)
  .openapi(alias.c, h)
  .openapi(ns.default.a, h);
const app = new OpenAPIHono();
app.route("/api", api);
"#,
            ),
        ],
    );
    let mut got = method_paths(&r);
    got.sort();
    assert_eq!(
        got,
        vec!["GET /api/a", "GET /api/b", "POST /api/c"],
        "{:?}",
        r.warnings
    );
    assert!(
        !r.warnings
            .iter()
            .any(|w| w.contains("could not be resolved")),
        "{:?}",
        r.warnings
    );
}

const WRAP_FACTORY: &str = "import { OpenAPIHono } from '@hono/zod-openapi';\nconst wrap = <A>(app: A): A => app;\nexport const createOpenAPIHono = <E extends Env = Env>(init = {}) =>\n  wrap(new OpenAPIHono<E>({ ...init }));\n";

#[test]
fn hono_factory_wrapping_the_constructor_in_another_call_is_recognised() {
    let r = scan_sources(
        "hono",
        &[
            ("src/factory.ts", WRAP_FACTORY),
            (
                "src/sub.ts",
                "import { createOpenAPIHono } from './factory';\nexport const sub = createOpenAPIHono();\nsub.post('/items', (c) => c.text('ok'));\n",
            ),
            (
                "src/index.ts",
                "import { createOpenAPIHono } from './factory';\nimport { sub } from './sub';\nconst app = createOpenAPIHono();\napp.get('/health', (c) => c.text('ok'));\napp.route('/api', sub);\nexport default app;\n",
            ),
        ],
    );
    assert_eq!(
        method_paths(&r),
        vec!["POST /api/items", "GET /health"],
        "{:?}",
        r.warnings
    );
}

#[test]
fn hono_alias_from_tsconfig_paths_extends_and_jsonc() {
    let factory = "import { OpenAPIHono } from '@hono/zod-openapi';\nexport const createOpenAPIHono = () => new OpenAPIHono();\n";
    let r = scan_sources(
        "hono",
        &[
            // JSONC: comments and trailing commas; `paths` come from a base config.
            (
                "tsconfig.base.json",
                "{\n  // aliases\n  \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@app/*\": [\"app/src/*\"], }, },\n}\n",
            ),
            (
                "app/tsconfig.json",
                "{ /* x */ \"extends\": \"../tsconfig.base\" }\n",
            ),
            ("app/src/common/openApiHono.ts", factory),
            (
                "app/src/common/index.ts",
                "export { createOpenAPIHono } from './openApiHono';\n",
            ),
            (
                "app/src/sub.ts",
                "import { createOpenAPIHono } from '@app/common';\nexport const sub = createOpenAPIHono();\nsub.post('/items', (c) => c.text('ok'));\n",
            ),
            (
                "app/src/index.ts",
                "import { createOpenAPIHono } from './common';\nimport { sub } from '@app/sub';\nconst app = createOpenAPIHono();\napp.get('/health', (c) => c.text('ok'));\napp.route('/api', sub);\nexport default app;\n",
            ),
        ],
    );
    assert_eq!(
        method_paths(&r),
        vec!["POST /api/items", "GET /health"],
        "{:?}",
        r.warnings
    );
}

#[test]
fn hono_alias_without_tsconfig_falls_back_to_src() {
    let r = scan_sources(
        "hono",
        &[
            (
                "src/sub.ts",
                "import { Hono } from 'hono';\nexport const sub = new Hono();\nsub.get('/x', (c) => c.text('x'));\n",
            ),
            (
                "src/index.ts",
                "import { Hono } from 'hono';\nimport { sub } from '@/sub';\nconst app = new Hono();\napp.route('/api', sub);\nexport default app;\n",
            ),
        ],
    );
    assert_eq!(method_paths(&r), vec!["GET /api/x"], "{:?}", r.warnings);
}

#[test]
fn hono_instance_returned_from_function_keeps_mount_prefix() {
    let r = scan_sources(
        "hono",
        &[
            (
                "src/trial.ts",
                "import { Hono } from 'hono';\nexport function createTrialRoutes() {\n  return new Hono()\n    .post('/sessions', (c) => c.text('a'))\n    .all('/x', (c) => c.text('b'));\n}\nexport const trial = createTrialRoutes();\n",
            ),
            (
                "src/arrow.ts",
                "import { Hono } from 'hono';\nexport const makeArrow = () => new Hono().get('/ping', (c) => c.text('p'));\nexport const arrow = makeArrow();\n",
            ),
            (
                "src/index.ts",
                "import { Hono } from 'hono';\nimport { trial } from './trial';\nimport { arrow } from './arrow';\nconst app = new Hono();\nconst routes = app.route('/v1/trial', trial).route('/v1/arrow', arrow);\nexport default app;\n",
            ),
        ],
    );
    let mp = method_paths(&r);
    assert!(
        mp.contains(&"POST /v1/trial/sessions".to_string()),
        "{:?}",
        mp
    );
    assert!(mp.contains(&"GET /v1/arrow/ping".to_string()), "{:?}", mp);
    assert!(
        !mp.iter().any(|x| x == "POST /sessions" || x == "GET /ping"),
        "{:?}",
        mp
    );
}

#[test]
fn hono_uncalled_factory_routes_are_not_emitted() {
    let r = scan_sources(
        "hono",
        &[
            (
                "src/trial.ts",
                "import { Hono } from 'hono';\nexport function stubResource(name: string) {\n  return new Hono().get('/:id', (c) => c.text(name));\n}\nexport const trial = new Hono().post('/sessions', (c) => c.text('ok'));\n",
            ),
            (
                "src/index.ts",
                "import { Hono } from 'hono';\nimport { trial } from './trial';\nconst app = new Hono();\napp.route('/v1/trial', trial);\nexport default app;\n",
            ),
        ],
    );
    assert_eq!(method_paths(&r), vec!["POST /v1/trial/sessions"]);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("factory function that is never called")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn hono_called_factory_is_mounted_per_call_site() {
    let r = scan_sources(
        "hono",
        &[
            (
                "src/res.ts",
                "import { Hono } from 'hono';\nexport function resource(n: string) {\n  return new Hono().get('/:id', (c) => c.text(n));\n}\n",
            ),
            (
                "src/index.ts",
                "import { Hono } from 'hono';\nimport { resource } from './res';\nconst app = new Hono();\napp.route('/a', resource('a'));\napp.route('/b', resource('b'));\nexport default app;\n",
            ),
        ],
    );
    assert_eq!(
        method_paths(&r),
        vec!["GET /a/{id}", "GET /b/{id}"],
        "{:?}",
        r.warnings
    );
}

#[test]
fn hono_duplicate_unconnected_createroute_is_listed_in_a_warning() {
    let r = scan_sources(
        "hono",
        &[
            (
                "a.routes.ts",
                "import { createRoute } from '@hono/zod-openapi';\nexport const a = createRoute({ method: 'get', path: '/' });\n",
            ),
            (
                "b.routes.ts",
                "import { createRoute } from '@hono/zod-openapi';\nexport const b = createRoute({ method: 'get', path: '/' });\n",
            ),
        ],
    );
    assert!(
        r.warnings.iter().any(|w| w.contains("share `GET /`")
            && w.contains("a.routes.ts:2")
            && w.contains("b.routes.ts:2")),
        "{:?}",
        r.warnings
    );
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("No Hono app instance") && w.contains("factory function")),
        "{:?}",
        r.warnings
    );
}

#[test]
fn hono_unresolved_mount_names_the_cause_and_is_not_duplicated() {
    let r = scan_sources(
        "hono",
        &[
            (
                "src/ranking.ts",
                "import { make } from './nowhere';\nexport const ranking = make();\nranking.get('/r', (c) => c.text('r'));\n",
            ),
            (
                "src/index.ts",
                "import { Hono } from 'hono';\nimport { ranking } from './ranking';\nimport { other } from '@/other';\nconst app = new Hono();\napp.route('/a', other);\napp.route('/b', other);\napp.route('/c', ranking);\nexport default app;\n",
            ),
        ],
    );
    let w: Vec<&String> = r
        .warnings
        .iter()
        .filter(|w| w.contains("could not resolve mounted app `other`"))
        .collect();
    assert_eq!(w.len(), 1, "{:?}", r.warnings);
    assert!(
        w[0].contains("alias") && w[0].contains("also mounted at line"),
        "{}",
        w[0]
    );
}
