//! Tests for MCP, mock server, OpenAPI, Postman/Bruno importers and HTML docs.

use ajiro_api::config::Project;
use ajiro_api::mcp::{handle_message, McpState};
use ajiro_api::mock::{self, MockOptions};
use ajiro_api::project::load_endpoints;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/integrations")
        .join(name)
}

/// Create a temp project with `ajiro-api.yaml` and the given files.
fn temp_project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ajiro-api.yaml"), "version: 1\n").unwrap();
    for (rel, text) in files {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    dir
}

fn load(dir: &Path) -> Project {
    Project::discover(dir).unwrap()
}

const GET_USER: &str = r#"version: 1
id: users.get
summary: Get a user
request:
  method: GET
  path: /users/{id}
  params:
    id: "1"
tests:
  - name: ok
    expect:
      status: 200
mock:
  default:
    status: 200
    body:
      id: "{{ params.id }}"
      q: "{{ query.x }}"
      uuid: "{{ $uuid }}"
      at: "{{ $isoDate }}"
  scenarios:
    notfound:
      status: 404
      body:
        error: nope
    slow:
      status: 200
      delay_ms: 5
      body: { slow: true }
"#;

const GET_ME: &str = r#"version: 1
id: users.me
request:
  method: GET
  path: /users/me
tests: []
mock:
  default:
    status: 200
    body: { me: true }
"#;

const POST_USER: &str = r#"version: 1
id: users.create
request:
  method: POST
  path: /users
  body: { name: Bob }
tests: []
mock:
  default:
    status: 201
    headers:
      X-Created: "{{ body.name }}"
    body:
      echo: "{{ body.name }}"
      n: "{{ body.n }}"
"#;

const DELETE_USER: &str = r#"version: 1
id: users.delete
request:
  method: DELETE
  path: /users/{id}
tests: []
"#;

fn users_project() -> tempfile::TempDir {
    temp_project(&[
        ("api/users/get-users-id.yaml", GET_USER),
        ("api/users/get-users-me.yaml", GET_ME),
        ("api/users/post-users.yaml", POST_USER),
        ("api/users/delete-users-id.yaml", DELETE_USER),
    ])
}

// ------------------------------------------------------------------ MCP ---

fn rpc(state: &mut McpState, msg: Value) -> Value {
    handle_message(state, &msg).expect("expected a response")
}

fn call(state: &mut McpState, id: u64, name: &str, args: Value) -> Value {
    rpc(
        state,
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } }),
    )
}

#[test]
fn mcp_initialize_and_negotiation() {
    let dir = users_project();
    let mut st = McpState::new(dir.path());
    let r = rpc(
        &mut st,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {} } }),
    );
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "ajiro-api");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert!(r["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("scan"));
    let r = rpc(
        &mut st,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }),
    );
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    // notifications get no response
    assert!(handle_message(
        &mut st,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .is_none());
    assert!(handle_message(
        &mut st,
        &json!({ "jsonrpc": "2.0", "method": "whatever/else" })
    )
    .is_none());
    // ping and unknown methods
    let r = rpc(
        &mut st,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" }),
    );
    assert_eq!(r["result"], json!({}));
    let r = rpc(
        &mut st,
        json!({ "jsonrpc": "2.0", "id": 4, "method": "nope/nope" }),
    );
    assert_eq!(r["error"]["code"], -32601);
    assert_eq!(r["id"], 4);
}

#[test]
fn mcp_tools_list_and_calls() {
    let dir = users_project();
    let mut st = McpState::new(dir.path());
    let r = rpc(
        &mut st,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    );
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for n in ["scan", "test", "list", "validate", "get_endpoint", "schema"] {
        assert!(names.contains(&n), "missing tool {}", n);
    }
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(!t["description"].as_str().unwrap().is_empty());
    }

    let r = call(&mut st, 2, "list", json!({}));
    assert!(r["result"].get("isError").is_none());
    let eps = r["result"]["structuredContent"]["endpoints"]
        .as_array()
        .unwrap();
    assert_eq!(eps.len(), 4);
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(serde_json::from_str::<Value>(text).is_ok());

    let r = call(&mut st, 3, "validate", json!({}));
    assert_eq!(r["result"]["structuredContent"]["ok"], true);

    let r = call(&mut st, 4, "get_endpoint", json!({ "id": "users.create" }));
    let sc = &r["result"]["structuredContent"];
    assert!(sc["yaml"].as_str().unwrap().contains("path: /users"));
    assert!(sc["file"].as_str().unwrap().ends_with("post-users.yaml"));

    let r = call(&mut st, 5, "get_endpoint", json!({ "id": "nope" }));
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("users.create"));

    let r = call(&mut st, 6, "schema", json!({ "which": "config" }));
    assert!(r["result"]["structuredContent"].is_object());

    // unknown tool is a protocol error
    let r = call(&mut st, 7, "bogus", json!({}));
    assert_eq!(r["error"]["code"], -32602);
}

#[test]
fn mcp_validate_reports_problems_and_rediscovers_per_call() {
    let dir = users_project();
    let mut st = McpState::new(dir.path());
    let r = call(&mut st, 1, "validate", json!({}));
    assert_eq!(r["result"]["structuredContent"]["ok"], true);
    std::fs::write(
        dir.path().join("api/users/broken.yaml"),
        "version: 1\nid: [\n",
    )
    .unwrap();
    let r = call(&mut st, 2, "validate", json!({}));
    assert_eq!(r["result"]["structuredContent"]["ok"], false);
    // no project at all -> tool error, not a crash
    let empty = tempfile::tempdir().unwrap();
    let mut st2 = McpState::new(empty.path());
    let r = call(&mut st2, 3, "list", json!({}));
    // temp dirs may sit below a project in odd setups; accept either outcome but never a protocol error
    assert!(r.get("error").is_none());
}

#[test]
fn mcp_binary_over_pipes() {
    let dir = users_project();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ajiro-api"))
        .arg("-C")
        .arg(dir.path())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2024-11-05" } })).unwrap();
        writeln!(
            stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .unwrap();
        writeln!(
            stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })
        )
        .unwrap();
        writeln!(stdin, "not json").unwrap();
        stdin.flush().unwrap();
    }
    drop(child.stdin.take());
    let lines: Vec<String> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| l.unwrap())
        .collect();
    child.wait().unwrap();
    assert_eq!(lines.len(), 3, "got: {:?}", lines);
    let a: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(a["result"]["protocolVersion"], "2024-11-05");
    let b: Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(b["id"], 2);
    assert!(b["result"]["tools"].as_array().unwrap().len() >= 6);
    let c: Value = serde_json::from_str(&lines[2]).unwrap();
    assert_eq!(c["error"]["code"], -32700);
}

// ----------------------------------------------------------------- mock ---

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("not json ({}): {}", e, self.body))
    }
}

fn wrap(mut r: ureq::http::Response<ureq::Body>) -> Resp {
    Resp {
        status: r.status().as_u16(),
        headers: r
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        body: r.body_mut().read_to_string().unwrap(),
    }
}

fn get(url: &str, headers: &[(&str, &str)]) -> Resp {
    let mut req = agent().get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    wrap(req.call().unwrap())
}

fn start_mock(dir: &Path, scenario: Option<&str>) -> mock::MockHandle {
    mock::start(
        load(dir),
        MockOptions {
            host: "127.0.0.1".into(),
            port: 0,
            scenario: scenario.map(str::to_string),
            delay_ms: None,
        },
    )
    .unwrap()
}

#[test]
fn mock_matches_params_and_templates() {
    let dir = users_project();
    let m = start_mock(dir.path(), None);
    let r = get(&format!("{}/users/123?x=hello", m.url()), &[]);
    assert_eq!(r.status, 200);
    assert!(r
        .header("content-type")
        .unwrap()
        .starts_with("application/json"));
    let v = r.json();
    assert_eq!(v["id"], "123");
    assert_eq!(v["q"], "hello");
    assert_eq!(v["uuid"].as_str().unwrap().len(), 36);
    assert!(v["at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));

    // static segment beats param
    let r = get(&format!("{}/users/me", m.url()), &[]);
    assert_eq!(r.json(), json!({ "me": true }));

    // body templating keeps types; response header templating
    let r = wrap(
        agent()
            .post(&format!("{}/users", m.url()))
            .send(r#"{"name":"Zed","n":5}"#)
            .unwrap(),
    );
    assert_eq!(r.status, 201);
    assert_eq!(r.json(), json!({ "echo": "Zed", "n": 5 }));
    assert_eq!(r.header("x-created"), Some("Zed"));

    // HEAD falls back to GET
    let h = wrap(
        agent()
            .head(&format!("{}/users/me", m.url()))
            .call()
            .unwrap(),
    );
    assert_eq!(h.status, 200);
    assert!(h.body.is_empty());
}

#[test]
fn mock_scenarios_header_query_and_flag() {
    let dir = users_project();
    let m = start_mock(dir.path(), None);
    let r = get(
        &format!("{}/users/1", m.url()),
        &[("X-Ajiro-Scenario", "notfound")],
    );
    assert_eq!(r.status, 404);
    assert_eq!(r.json()["error"], "nope");
    let r = get(&format!("{}/users/1?__scenario=notfound", m.url()), &[]);
    assert_eq!(r.status, 404);
    // unknown scenario falls back to default
    let r = get(
        &format!("{}/users/1", m.url()),
        &[("X-Ajiro-Scenario", "zzz")],
    );
    assert_eq!(r.status, 200);
    // scenario without the endpoint defining it -> default (users.me has none)
    let r = get(
        &format!("{}/users/me", m.url()),
        &[("X-Ajiro-Scenario", "notfound")],
    );
    assert_eq!(r.status, 200);

    let m2 = start_mock(dir.path(), Some("notfound"));
    let r = get(&format!("{}/users/1", m2.url()), &[]);
    assert_eq!(r.status, 404);
    // header overrides the flag
    let r = get(
        &format!("{}/users/1", m2.url()),
        &[("X-Ajiro-Scenario", "slow")],
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), json!({ "slow": true }));
}

#[test]
fn mock_404_501_cors_and_listing() {
    let dir = users_project();
    let m = start_mock(dir.path(), None);

    let r = get(&format!("{}/nothing/here", m.url()), &[]);
    assert_eq!(r.status, 404);
    assert!(r.json()["close_matches"].is_array());
    let r = get(&format!("{}/users/1/extra", m.url()), &[]);
    assert_eq!(r.status, 404);
    assert!(r.json()["close_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s.as_str().unwrap().contains("/users/{id}")));

    let r = wrap(
        agent()
            .delete(&format!("{}/users/1", m.url()))
            .call()
            .unwrap(),
    );
    assert_eq!(r.status, 501);
    let v = r.json();
    assert_eq!(v["error"], "no mock defined");
    assert_eq!(v["endpoint"], "users.delete");
    assert!(v["hint"].as_str().unwrap().contains("mock.default"));

    let r = wrap(
        agent()
            .options(&format!("{}/users", m.url()))
            .header("Origin", "http://example.com")
            .header("Access-Control-Request-Method", "POST")
            .header(
                "Access-Control-Request-Headers",
                "content-type,authorization",
            )
            .call()
            .unwrap(),
    );
    assert_eq!(r.status, 204);
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    assert!(r
        .header("access-control-allow-methods")
        .unwrap()
        .contains("POST"));
    assert_eq!(
        r.header("access-control-allow-headers"),
        Some("content-type,authorization")
    );

    let r = get(&format!("{}/__ajiro/endpoints", m.url()), &[]);
    let v = r.json();
    assert_eq!(v["endpoints"].as_array().unwrap().len(), 4);
}

#[test]
fn mock_hot_reload() {
    let dir = users_project();
    let m = start_mock(dir.path(), None);
    assert_eq!(
        get(&format!("{}/users/me", m.url()), &[]).json(),
        json!({ "me": true })
    );
    assert_eq!(get(&format!("{}/ping", m.url()), &[]).status, 404);

    std::fs::write(
        dir.path().join("api/users/get-users-me.yaml"),
        GET_ME.replace("me: true", "me: false, v: 2"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("api/get-ping.yaml"),
        "version: 1\nid: ping\nrequest: { method: GET, path: /ping }\nmock:\n  default: { status: 200, body: pong }\n",
    )
    .unwrap();
    assert_eq!(
        get(&format!("{}/users/me", m.url()), &[]).json(),
        json!({ "me": false, "v": 2 })
    );
    let r = get(&format!("{}/ping", m.url()), &[]);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, "pong");
    std::fs::remove_file(dir.path().join("api/get-ping.yaml")).unwrap();
    assert_eq!(get(&format!("{}/ping", m.url()), &[]).status, 404);
}

// -------------------------------------------------------------- OpenAPI ---

fn set_of(doc: &Value) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    for (p, item) in doc["paths"].as_object().unwrap() {
        for (m, _) in item.as_object().unwrap() {
            s.insert(format!("{} {}", m.to_uppercase(), p));
        }
    }
    s
}

#[test]
fn openapi_import_creates_files_and_respects_conflicts() {
    let dir = temp_project(&[]);
    let project = load(dir.path());
    let text = std::fs::read_to_string(fixture("petstore.yaml")).unwrap();
    let report = ajiro_api::openapi::import_str(&project, &text, false).unwrap();
    assert_eq!(report.created.len(), 4, "{:?}", report);
    assert!(report.skipped.is_empty());

    let (eps, errs) = load_endpoints(&project);
    assert!(errs.is_empty(), "{:?}", errs);
    let by = |m: &str, p: &str| {
        eps.iter()
            .find(|e| e.def.request.method == m && e.def.request.path == p)
            .unwrap()
    };

    let list = by("GET", "/pets");
    assert_eq!(list.def.id, "list.pets");
    assert_eq!(list.def.request.query.get("limit"), Some(&json!(20)));
    assert_eq!(list.def.request.query.get("tag"), Some(&Value::Null));
    let mock = list.def.mock.as_ref().unwrap();
    let body = mock.default.as_ref().unwrap().body.as_ref().unwrap();
    assert_eq!(body[0]["name"], "Rex");
    assert_eq!(body[0]["email"], "user@example.com");
    assert_eq!(body[0]["id"], 1);
    assert_eq!(list.def.tests[0].name, "returns 200");
    assert_eq!(list.def.summary.as_deref(), Some("List pets"));

    let create = by("POST", "/pets");
    assert_eq!(create.def.request.body.as_ref().unwrap()["name"], "Rex");
    assert_eq!(
        create
            .def
            .mock
            .as_ref()
            .unwrap()
            .default
            .as_ref()
            .unwrap()
            .status,
        201
    );
    assert_eq!(
        create.def.mock.as_ref().unwrap().scenarios["422"]
            .body
            .as_ref()
            .unwrap()["error"],
        "name is required"
    );

    let one = by("GET", "/pets/{petId}");
    assert_eq!(one.def.id, "get.pet.by.id");
    assert!(one.def.request.headers.contains_key("X-Request-Id"));
    assert!(one.def.mock.as_ref().unwrap().scenarios.contains_key("404"));
    let del = by("DELETE", "/pets/{petId}");
    assert_eq!(del.def.id, "pets.delete");
    assert_eq!(del.def.tests[0].name, "returns 204");

    // existing files are left untouched without --overwrite
    let file = project.root.join(&list.rel_path);
    let mut def = list.def.clone();
    def.tests.push(ajiro_api::model::TestCase {
        name: "mine".into(),
        ..Default::default()
    });
    def.summary = Some("custom".into());
    std::fs::write(&file, ajiro_api::importers::write::to_yaml(&def).unwrap()).unwrap();
    let report = ajiro_api::openapi::import_str(&project, &text, false).unwrap();
    assert_eq!(report.created.len(), 0);
    assert_eq!(report.skipped.len(), 4);
    assert!(std::fs::read_to_string(&file).unwrap().contains("custom"));

    // --overwrite replaces summary/request/mock but keeps tests and id
    let report = ajiro_api::openapi::import_str(&project, &text, true).unwrap();
    assert_eq!(report.updated.len(), 4);
    let (eps, _) = load_endpoints(&project);
    let e = eps
        .iter()
        .find(|e| e.def.request.path == "/pets" && e.def.request.method == "GET")
        .unwrap();
    assert_eq!(e.def.summary.as_deref(), Some("List pets"));
    assert_eq!(e.def.id, "list.pets");
    assert!(e.def.tests.iter().any(|t| t.name == "mine"));
    assert!(std::fs::read_to_string(&file)
        .unwrap()
        .starts_with("# yaml-language-server"));
}

#[test]
fn openapi_round_trip() {
    let a = temp_project(&[]);
    std::fs::write(
        a.path().join("ajiro-api.yaml"),
        "version: 1\nenvironments:\n  local:\n    base_url: http://localhost:3000\n",
    )
    .unwrap();
    let pa = load(a.path());
    let text = std::fs::read_to_string(fixture("petstore.yaml")).unwrap();
    ajiro_api::openapi::import_str(&pa, &text, false).unwrap();
    let doc1 = ajiro_api::openapi::export(&pa, Some("Petstore"));
    assert_eq!(doc1["openapi"], "3.1.0");
    assert_eq!(doc1["info"]["title"], "Petstore");
    assert_eq!(doc1["servers"][0]["url"], "http://localhost:3000");
    let op = &doc1["paths"]["/pets/{petId}"]["get"];
    assert_eq!(op["operationId"], "get.pet.by.id");
    assert_eq!(op["parameters"][0]["in"], "path");
    assert_eq!(op["parameters"][0]["required"], true);
    assert!(op["x-ajiro-file"]
        .as_str()
        .unwrap()
        .contains("get-pets-pet"));
    assert_eq!(
        op["responses"]["404"]["content"]["application/json"]["example"]["error"],
        "pet not found"
    );
    assert_eq!(
        doc1["paths"]["/pets"]["post"]["requestBody"]["content"]["application/json"]["example"]
            ["name"],
        "Rex"
    );

    let b = temp_project(&[]);
    let pb = load(b.path());
    ajiro_api::openapi::import_str(&pb, &serde_json::to_string(&doc1).unwrap(), false).unwrap();
    let doc2 = ajiro_api::openapi::export(&pb, Some("Petstore"));
    assert_eq!(set_of(&doc1), set_of(&doc2));
    assert_eq!(set_of(&doc1).len(), 4);
    // ids survive the round trip
    assert_eq!(
        doc1["paths"]["/pets"]["get"]["operationId"],
        doc2["paths"]["/pets"]["get"]["operationId"]
    );
}

#[test]
fn openapi_export_bearer_and_test_schema() {
    let dir = temp_project(&[
        (
            "api/orders/get-orders.yaml",
            r#"version: 1
id: orders.list
tags: [shop]
request:
  method: GET
  path: /orders
  headers:
    Authorization: "Bearer {{ env.TOKEN }}"
    X-Tenant: acme
    Accept: application/json
  query:
    page: 1
tests:
  - name: ok
    expect:
      status: 200
      body:
        schema: ../schemas/orders.json
source:
  framework: express
  file: src/orders.ts
  line: 12
"#,
        ),
        (
            "api/schemas/orders.json",
            r##"{"$schema":"http://json-schema.org/draft-07/schema#","type":"array","items":{"type":"object","properties":{"id":{"type":"integer"}}}}"##,
        ),
    ]);
    let doc = ajiro_api::openapi::export(&load(dir.path()), None);
    let op = &doc["paths"]["/orders"]["get"];
    assert_eq!(op["tags"][0], "shop");
    assert_eq!(op["security"][0]["bearerAuth"], json!([]));
    assert_eq!(
        doc["components"]["securitySchemes"]["bearerAuth"]["scheme"],
        "bearer"
    );
    let names: Vec<&str> = op["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["page", "X-Tenant"]);
    assert_eq!(
        op["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        "array"
    );
    assert!(
        op["responses"]["200"]["content"]["application/json"]["schema"]
            .get("$schema")
            .is_none()
    );
    assert_eq!(op["x-ajiro-source"]["line"], 12);
}

// ------------------------------------------------------------- importers ---

#[test]
fn postman_import() {
    let dir = temp_project(&[]);
    let project = load(dir.path());
    let text = std::fs::read_to_string(fixture("postman.json")).unwrap();
    let report = ajiro_api::importers::postman::import_str(&project, &text, false).unwrap();
    assert_eq!(report.created.len(), 3, "{:?}", report);
    assert_eq!(
        report.suggested_variables["local"]["baseUrl"],
        "http://localhost:3000"
    );
    assert_eq!(report.suggested_variables["local"]["token"], "dev-token");
    assert!(
        report.warnings.iter().any(|w| w.contains("scripts")),
        "{:?}",
        report.warnings
    );

    let (eps, errs) = load_endpoints(&project);
    assert!(errs.is_empty(), "{:?}", errs);
    let by = |m: &str, p: &str| {
        eps.iter()
            .find(|e| e.def.request.method == m && e.def.request.path == p)
            .unwrap()
    };

    let get_user = by("GET", "/users/{id}");
    assert_eq!(get_user.def.id, "users.get");
    assert_eq!(get_user.def.tags, vec!["Users"]);
    assert_eq!(get_user.def.summary.as_deref(), Some("Get user"));
    assert_eq!(get_user.def.request.params["id"], json!("42"));
    assert_eq!(get_user.def.request.query["verbose"], json!("true"));
    assert!(!get_user.def.request.query.contains_key("off"));
    assert!(!get_user.def.request.headers.contains_key("X-Debug"));
    assert_eq!(
        get_user.def.request.headers["Authorization"],
        "Bearer {{ token }}"
    );
    let mock = get_user.def.mock.as_ref().unwrap();
    assert_eq!(
        mock.default.as_ref().unwrap().body.as_ref().unwrap()["name"],
        "Alice"
    );
    assert_eq!(mock.scenarios["404"].status, 404);
    assert_eq!(get_user.def.tests[0].name, "returns 200");

    let create = by("POST", "/users");
    assert_eq!(
        create.def.request.body.as_ref().unwrap(),
        &json!({ "email": "{{ email }}", "name": "Bob" })
    );
    assert_eq!(
        create.def.request.headers["Content-Type"],
        "application/json"
    );
    assert_eq!(
        create
            .def
            .mock
            .as_ref()
            .unwrap()
            .default
            .as_ref()
            .unwrap()
            .status,
        201
    );

    let login = by("POST", "/auth/login");
    assert!(!login.def.request.headers.contains_key("Authorization"));
    assert_eq!(
        login.def.request.body.as_ref().unwrap(),
        &json!({ "user": "{{ user }}" })
    );

    // re-import: nothing new
    let again = ajiro_api::importers::postman::import_str(&project, &text, false).unwrap();
    assert_eq!(again.created.len(), 0);
    assert_eq!(again.skipped.len(), 3);
}

#[test]
fn bruno_import() {
    let dir = temp_project(&[]);
    let project = load(dir.path());
    let report =
        ajiro_api::importers::bruno::import_dir(&project, &fixture("bruno"), false).unwrap();
    assert_eq!(report.created.len(), 2, "{:?}", report);
    assert_eq!(
        report.suggested_variables["local"]["baseUrl"],
        "http://localhost:3000"
    );
    assert!(report.suggested_variables["local"].contains_key("token"));
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("script:post-response")),
        "{:?}",
        report.warnings
    );
    assert!(
        report.warnings.iter().any(|w| w.contains("res.body.id")),
        "{:?}",
        report.warnings
    );

    let (eps, errs) = load_endpoints(&project);
    assert!(errs.is_empty(), "{:?}", errs);
    let by = |m: &str, p: &str| {
        eps.iter()
            .find(|e| e.def.request.method == m && e.def.request.path == p)
            .unwrap()
    };

    let create = by("POST", "/users");
    assert_eq!(create.def.summary.as_deref(), Some("Create user"));
    assert_eq!(create.def.description.as_deref(), Some("Creates a user."));
    assert_eq!(create.def.tags, vec!["users"]);
    assert_eq!(
        create.def.request.headers["Authorization"],
        "Bearer {{ token }}"
    );
    assert_eq!(create.def.request.headers["X-Trace"], "abc");
    assert!(!create.def.request.headers.contains_key("X-Disabled"));
    let body = create.def.request.body.as_ref().unwrap();
    assert_eq!(body["email"], "{{ email }}");
    assert_eq!(body["profile"]["tags"], json!(["a", "b"]));
    assert_eq!(create.def.tests[0].name, "returns 201");
    assert_eq!(
        report.suggested_variables["local"]["email"],
        "carol@example.com"
    );

    let get = by("GET", "/users/{id}");
    assert_eq!(get.def.request.params["id"], json!("7"));
    assert_eq!(get.def.request.query["expand"], json!("1"));
    assert!(!get.def.request.query.contains_key("debug"));
    let m = &get.def.tests[0].expect.body.as_ref().unwrap().match_;
    assert_eq!(m["$.name"], json!("Alice"));
}

// ----------------------------------------------------------------- docs ---

#[test]
fn html_docs() {
    let dir = temp_project(&[
        (
            "api/users/get-users-id.yaml",
            r#"version: 1
id: users.get
summary: "<script>alert('x')</script> ユーザー取得"
description: "Reads a user & more"
task: TSK-012
request:
  method: GET
  path: /users/{id}
  params: { id: "42" }
  query: { verbose: "true" }
  headers:
    Authorization: "Bearer {{ env.TOKEN }}"
    X-Api-Key: "{{ env.KEY }}"
    X-Plain: visible
source: { framework: express, file: src/users.ts, line: 9 }
tests:
  - name: found
    expect: { status: 200 }
  - name: missing
    expect: { status: 404 }
mock:
  default:
    status: 200
    body: { id: 42, note: "</pre><b>x</b>" }
  scenarios:
    notfound: { status: 404, body: { error: nope } }
"#,
        ),
        (
            "api/users/post-users.yaml",
            "version: 1\nid: users.create\nrequest:\n  method: POST\n  path: /users\n  body: { name: Bob }\ntests: []\n",
        ),
    ]);
    std::fs::write(
        dir.path().join("ajiro-api.yaml"),
        "version: 1\nenvironments:\n  local:\n    base_url: http://localhost:3000\n  stg:\n    base_url: https://stg.example.com\n",
    )
    .unwrap();
    let project = load(dir.path());
    let html = ajiro_api::htmldoc::generate(&project, Some("My <API>"), Some("stg")).unwrap();
    assert!(html.contains("<meta charset=\"utf-8\">"));
    assert!(html.contains("<title>My &lt;API&gt;</title>"));
    assert!(html.contains("/users/{id}"));
    assert!(html.contains("users.create"));
    assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; ユーザー取得"));
    assert!(!html.contains("<script>alert"));
    assert!(html.contains("&lt;/pre&gt;&lt;b&gt;x&lt;/b&gt;"));
    assert!(html.contains("TSK-012"));
    assert!(html.contains("src/users.ts:9"));
    assert!(html.contains("Bearer &lt;token&gt;"));
    assert!(html.contains("X-Api-Key: &lt;token&gt;"));
    assert!(html.contains("X-Plain: visible"));
    assert!(!html.contains("env.TOKEN"));
    assert!(html.contains("curl -X GET &#39;https://stg.example.com/users/42?verbose=true&#39;"));
    assert!(html.contains("scenario: notfound"));
    assert!(html.contains("Generated by Ajiro API"));
    assert!(html.contains("https://github.com/niti-lab/ajiro-api"));
    assert!(html.contains("prefers-color-scheme:dark"));
    assert!(!html.contains("src=\"http"));
    assert!(!html.contains("<link"));

    // unknown env is an error
    assert!(ajiro_api::htmldoc::generate(&project, None, Some("nope")).is_err());

    // through the CLI entry point
    let out = dir.path().join("out/docs.html");
    let args = ajiro_api::cli::DocsArgs {
        out: out.clone(),
        title: None,
        env: None,
    };
    assert_eq!(ajiro_api::htmldoc::run_cli(dir.path(), &args).unwrap(), 0);
    assert!(std::fs::read_to_string(out)
        .unwrap()
        .contains("Generated by Ajiro API"));
}
