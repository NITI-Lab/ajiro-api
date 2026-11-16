//! Integration tests for `ajiro_api::runner` against a local tiny_http server.

use ajiro_api::cli::TestArgs;
use ajiro_api::config::Project;
use ajiro_api::report::{TestOptions, TestReport, TestResult, TestStatus};
use ajiro_api::runner::{run_cli, run_tests};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SECRET: &str = "sup3r-s3cret-pw";
const TOKEN: &str = "tok-4f9a1c77";

// ---------------------------------------------------------------- server ---

fn respond(req: tiny_http::Request, status: u16, ct: &str, body: String, extra: &[(&str, &str)]) {
    let mut r = tiny_http::Response::from_string(body).with_status_code(status);
    r.add_header(tiny_http::Header::from_bytes("Content-Type", ct).unwrap());
    for (k, v) in extra {
        r.add_header(tiny_http::Header::from_bytes(*k, *v).unwrap());
    }
    let _ = req.respond(r);
}

fn json_resp(req: tiny_http::Request, status: u16, v: Value) {
    respond(req, status, "application/json", v.to_string(), &[]);
}

fn start_server() -> u16 {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let users: Arc<Mutex<HashMap<u64, Value>>> = Arc::new(Mutex::new(HashMap::new()));
    users.lock().unwrap().insert(
        1,
        json!({"id": 1, "name": "Alice", "email": "alice@example.com", "createdAt": "2024-01-01T00:00:00Z"}),
    );
    let next = Arc::new(Mutex::new(100u64));
    std::thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let users = users.clone();
            let next = next.clone();
            std::thread::spawn(move || {
                let url = req.url().to_string();
                let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
                let method = req.method().as_str().to_string();
                let auth = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.as_str().to_string());
                let ctype = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Content-Type"))
                    .map(|h| h.value.as_str().to_string());
                let xenv = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("X-Env"))
                    .map(|h| h.value.as_str().to_string());
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let parsed: Option<Value> = serde_json::from_str(&body).ok();

                match (method.as_str(), path) {
                    ("POST", "/login") => {
                        let ok = parsed
                            .as_ref()
                            .and_then(|b| b.get("password"))
                            .and_then(|p| p.as_str())
                            == Some(SECRET);
                        if ok {
                            json_resp(
                                req,
                                200,
                                json!({"token": TOKEN, "user": {"id": 7, "name": "Admin"}}),
                            )
                        } else {
                            json_resp(req, 401, json!({"error": "bad credentials"}))
                        }
                    }
                    ("POST", "/users") => {
                        if auth.as_deref() != Some(&format!("Bearer {}", TOKEN)) {
                            return json_resp(req, 401, json!({"error": "unauthorized"}));
                        }
                        let Some(mut b) = parsed else {
                            return json_resp(req, 400, json!({"error": "invalid json"}));
                        };
                        if b.get("name").is_none() {
                            return json_resp(req, 400, json!({"error": "name required"}));
                        }
                        let mut n = next.lock().unwrap();
                        *n += 1;
                        b["id"] = json!(*n);
                        b["createdAt"] = json!(format!("2024-02-{:02}T10:00:00Z", *n % 28 + 1));
                        users.lock().unwrap().insert(*n, b.clone());
                        json_resp(req, 201, b)
                    }
                    ("GET", p) if p.starts_with("/users/") => {
                        let id: u64 = p[7..].parse().unwrap_or(0);
                        match users.lock().unwrap().get(&id).cloned() {
                            Some(u) => json_resp(req, 200, u),
                            None => json_resp(req, 404, json!({"error": "not found"})),
                        }
                    }
                    ("GET", "/users-bad") => {
                        json_resp(req, 200, json!({"id": "x", "name": 5, "extra": {"a": 1}}))
                    }
                    ("GET", "/items") => json_resp(
                        req,
                        200,
                        json!({"items": [{"id": 1, "name": "apple", "price": 1.5, "tags": ["a", "b"]}, {"id": 2, "name": "pear", "price": 3, "tags": []}], "total": 2, "note": null}),
                    ),
                    ("GET", "/boom") => json_resp(req, 500, json!({"error": "boom"})),
                    ("GET", "/redirect") => respond(
                        req,
                        302,
                        "text/plain",
                        String::new(),
                        &[("Location", "/users/1")],
                    ),
                    ("GET", "/slow") => {
                        std::thread::sleep(Duration::from_millis(300));
                        json_resp(req, 200, json!({"ok": true}))
                    }
                    ("GET", "/text") => respond(
                        req,
                        200,
                        "text/plain; charset=utf-8",
                        "hello world".into(),
                        &[],
                    ),
                    ("GET", "/leak") => json_resp(
                        req,
                        200,
                        json!({"debug": format!("pw={}", SECRET), "ok": true}),
                    ),
                    (_, "/echo") => json_resp(
                        req,
                        200,
                        json!({"method": method, "query": query, "contentType": ctype, "body": body, "json": parsed, "xenv": xenv}),
                    ),
                    _ => json_resp(req, 404, json!({"error": "no route"})),
                }
            });
        }
    });
    port
}

// --------------------------------------------------------------- fixture ---

struct Fx {
    dir: tempfile::TempDir,
    port: u16,
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

impl Fx {
    fn new(files: &[(&str, &str)]) -> Fx {
        let port = start_server();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "ajiro-api.yaml",
            &format!(
                r#"version: 1
default_env: local
redact: [x-internal]
environments:
  local:
    base_url: http://127.0.0.1:{port}/
    variables:
      adminPassword: "{{{{ env.ADMIN_PASSWORD }}}}"
      greeting: hello
    headers:
      X-Env: local
  staging:
    base_url: http://127.0.0.1:{port}
"#
            ),
        );
        write(root, ".env", &format!("ADMIN_PASSWORD={}\n", SECRET));
        write(
            root,
            "api/schemas/user.json",
            r##"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "required": ["id", "name", "email"],
  "properties": {
    "id": {"$ref": "common.json#/definitions/id"},
    "name": {"type": "string"},
    "email": {"type": "string"}
  }
}"##,
        );
        write(
            root,
            "api/schemas/common.json",
            r##"{"definitions": {"id": {"type": "integer", "minimum": 1}}}"##,
        );
        for (rel, text) in files {
            write(root, &format!("api/{}", rel), text);
        }
        Fx { dir, port }
    }

    fn project(&self) -> Project {
        Project::load(&self.dir.path().join("ajiro-api.yaml")).unwrap()
    }

    fn run(&self) -> TestReport {
        self.run_with(TestOptions::default())
    }

    fn run_with(&self, opts: TestOptions) -> TestReport {
        run_tests(&self.project(), &opts).unwrap()
    }
}

fn find<'a>(r: &'a TestReport, id: &str, test: &str) -> &'a TestResult {
    r.results
        .iter()
        .find(|x| x.endpoint_id == id && x.test == test)
        .unwrap_or_else(|| {
            panic!(
                "no result for {} / {}: {:?}",
                id,
                test,
                r.results
                    .iter()
                    .map(|x| (&x.endpoint_id, &x.test))
                    .collect::<Vec<_>>()
            )
        })
}

fn assert_pass(r: &TestReport, id: &str, test: &str) {
    let t = find(r, id, test);
    assert_eq!(
        t.status,
        TestStatus::Passed,
        "{} / {}: failures={:?} error={:?}",
        id,
        test,
        t.failures,
        t.error
    );
}

// ----------------------------------------------------------------- tests ---

const LOGIN: &str = r#"version: 1
id: auth.login
request:
  method: POST
  path: /login
  body:
    email: admin@example.com
    password: "{{ adminPassword }}"
tests:
  - name: logs in
    expect:
      status: 200
      body:
        match:
          $.token: { $type: string }
          $.user.id: 7
    capture:
      token: $.token
      adminId: $.user.id
  - name: wrong password
    request:
      body:
        password: nope
    expect:
      status: 401
      body:
        equals: { error: bad credentials }
"#;

const CREATE: &str = r#"version: 1
id: users.create
depends_on: [auth.login]
request:
  method: POST
  path: /users
  headers:
    Authorization: "Bearer {{ token }}"
  body:
    name: Bob
    email: "{{ $randomEmail }}"
    password: "{{ adminPassword }}"
    profile: { age: 30, city: Tokyo }
tests:
  - name: creates a user
    expect:
      status: 201
      headers:
        content-type: "/^application\\/json/"
      body:
        match:
          $.name: Bob
          $.profile.age: { $gte: 18 }
          $.id: { $gt: 100 }
        schema: ../schemas/user.json
    capture:
      userId: $.id
      created: $.createdAt
  - name: missing name is rejected
    request:
      body:
        name: null
    expect:
      status: 400
      body:
        equals: { error: name required }
  - name: no auth
    request:
      headers:
        Authorization: Bearer wrong
    expect:
      status: 401
"#;

const GET_USER: &str = r#"version: 1
id: users.get
depends_on: [users.create]
request:
  method: GET
  path: /users/{id}
tests:
  - name: fetches the created user
    request:
      params:
        id: "{{ userId }}"
    expect:
      status: 200
      body:
        match:
          $.id: "{{ userId }}"
          $.createdAt: "{{ created }}"
          $.profile.city: Tokyo
  - name: seed user equals with ignore
    request:
      params: { id: 1 }
    expect:
      status: 200
      ignore: ["$.id", "$.createdAt"]
      body:
        equals: { id: 999, name: Alice, email: alice@example.com, createdAt: whenever }
  - name: unknown user
    request:
      params: { id: 424242 }
    expect:
      status: 404
"#;

#[test]
fn chaining_templating_ordering_and_redaction() {
    // `zz-auth` sorts after `users`, so the dependency order must be what makes it work.
    let fx = Fx::new(&[
        ("zz-auth/post-login.yaml", LOGIN),
        ("users/post-users.yaml", CREATE),
        ("users/get-users-id.yaml", GET_USER),
    ]);
    let r = fx.run();
    assert!(
        r.ok,
        "{:#?}",
        r.results
            .iter()
            .filter(|t| t.status != TestStatus::Passed)
            .collect::<Vec<_>>()
    );
    assert_eq!(r.summary.total, 8);
    assert_eq!(r.summary.passed, 8);
    assert!(r.load_errors.is_empty());
    let ids: Vec<&str> = r.results.iter().map(|t| t.endpoint_id.as_str()).collect();
    assert_eq!(&ids[..2], ["auth.login", "auth.login"]);
    assert!(
        ids.iter().position(|i| *i == "users.create") < ids.iter().position(|i| *i == "users.get")
    );
    assert_eq!(r.base_url, format!("http://127.0.0.1:{}", fx.port));

    // numeric type kept for whole-string template (`$.id: "{{ userId }}"` passed above)
    let create = find(&r, "users.create", "creates a user");
    let req = create.request.as_ref().unwrap();
    assert_eq!(req.method, "POST");
    assert!(req.url.ends_with("/users"));
    assert_eq!(req.headers["Authorization"], "Bearer ***");
    assert_eq!(req.headers["X-Env"], "local");
    assert_eq!(req.body.as_ref().unwrap()["password"], "***");
    assert!(req.body.as_ref().unwrap()["email"]
        .as_str()
        .unwrap()
        .ends_with("@example.com"));
    let resp = create.response.as_ref().unwrap();
    assert_eq!(resp.status, 201);

    // no secret anywhere in the machine-readable report
    let all = serde_json::to_string(&r).unwrap();
    assert!(!all.contains(SECRET), "dotenv secret leaked");
    assert!(!all.contains(TOKEN), "captured token leaked");
    assert!(all.contains("***"));
    let login = find(&r, "auth.login", "logs in");
    assert_eq!(
        login.response.as_ref().unwrap().body.as_ref().unwrap()["token"],
        "***"
    );
}

#[test]
fn matchers_all_kinds() {
    let fx = Fx::new(&[(
        "items/get-items.yaml",
        r#"version: 1
id: items.list
request: { method: GET, path: /items }
tests:
  - name: all matchers pass
    expect:
      status: [200, 201]
      body:
        match:
          $.total: 2
          $["total"]: 2
          $.items[0].name: apple
          $.items[0].price: { $type: number }
          $.items[1].price: { $type: integer }
          $.items[0].id: { $exists: true }
          $.nothing: { $exists: false }
          $.note: { $type: "null" }
          $.items[0]["name"]: { $regex: "^app" }
          $.items[0].tags: { $contains: a }
          $.items: { $len: 2 }
          $.items[*].name: { $contains: pear }   # all matches, not just the first
          $.total: { $gt: 1 }
          $..total: { $len: { $gte: 1 } }
          $['total']: { $lt: 3 }
          $[ 'total' ]: { $lte: 2 }
          $.items[1].id: { $ne: 5 }
  - name: failing matchers report everything
    expect:
      status: 201
      body:
        match:
          $.total: 3
          $.missing: 1
          $.items[0].name: { $regex: "^z" }
          $.items: { $len: 5 }
          $["total"]: { $lt: 1 }
"#,
    )]);
    let r = fx.run();
    assert_pass(&r, "items.list", "all matchers pass");
    let t = find(&r, "items.list", "failing matchers report everything");
    assert_eq!(t.status, TestStatus::Failed);
    let checks: Vec<(&str, Option<&str>)> = t
        .failures
        .iter()
        .map(|f| (f.check.as_str(), f.path.as_deref()))
        .collect();
    assert_eq!(
        checks,
        vec![
            ("status", None),
            ("body.match", Some("$.total")),
            ("body.match", Some("$.missing")),
            ("body.match", Some("$.items[0].name")),
            ("body.match", Some("$.items")),
            ("body.match", Some("$[\"total\"]")),
        ]
    );
    let missing = &t.failures[2];
    assert_eq!(missing.actual, Value::Null);
    assert!(missing.message.contains("path not found"));
    assert_eq!(t.failures[1].expected, json!(3));
    assert_eq!(t.failures[1].actual, json!(2));
}

#[test]
fn schema_errors_have_jsonpaths_and_refs_resolve() {
    let fx = Fx::new(&[(
        "users/get-bad.yaml",
        r#"version: 1
id: users.bad
request: { method: GET, path: /users-bad }
tests:
  - name: schema violations
    expect:
      status: 200
      body:
        schema: ../schemas/user.json
  - name: schema by api/schemas relative path
    expect:
      body:
        schema: user.json
  - name: inline schema
    expect:
      body:
        schema: { type: object, properties: { extra: { type: object, properties: { a: { type: string } } } } }
  - name: missing schema file
    expect:
      body:
        schema: nope.json
"#,
    )]);
    let r = fx.run();
    let t = find(&r, "users.bad", "schema violations");
    assert_eq!(t.status, TestStatus::Failed);
    let paths: Vec<&str> = t
        .failures
        .iter()
        .filter_map(|f| f.path.as_deref())
        .collect();
    assert!(paths.contains(&"$.id"), "{:?}", t.failures);
    assert!(paths.contains(&"$.name"), "{:?}", t.failures);
    assert!(paths.contains(&"$"), "required email: {:?}", t.failures);
    assert!(t.failures.iter().all(|f| f.check == "body.schema"));
    let id = t
        .failures
        .iter()
        .find(|f| f.path.as_deref() == Some("$.id"))
        .unwrap();
    assert_eq!(id.actual, json!("x"));

    let t2 = find(&r, "users.bad", "schema by api/schemas relative path");
    assert_eq!(t2.status, TestStatus::Failed);
    let t3 = find(&r, "users.bad", "inline schema");
    assert_eq!(t3.failures.len(), 1);
    assert_eq!(t3.failures[0].path.as_deref(), Some("$.extra.a"));
    let t4 = find(&r, "users.bad", "missing schema file");
    assert!(t4.failures[0].message.contains("not found"));
}

#[test]
fn equals_with_ignore_and_diffs() {
    let fx = Fx::new(&[(
        "users/get-users-id.yaml",
        r#"version: 1
id: users.get
request: { method: GET, path: "/users/{id}", params: { id: 1 } }
tests:
  - name: ignored volatile fields
    expect:
      ignore: ["$.id", "$..createdAt"]
      body:
        equals: { id: 5, name: Alice, email: alice@example.com, createdAt: x }
  - name: mismatch
    expect:
      body:
        equals: { id: 1, name: Alicia, email: alice@example.com, createdAt: "2024-01-01T00:00:00Z", extra: true }
"#,
    )]);
    let r = fx.run();
    assert_pass(&r, "users.get", "ignored volatile fields");
    let t = find(&r, "users.get", "mismatch");
    assert_eq!(t.failures.len(), 1);
    let f = &t.failures[0];
    assert_eq!(f.check, "body.equals");
    assert_eq!(f.path.as_deref(), Some("$.name"));
    assert_eq!(f.expected, json!("Alicia"));
    assert_eq!(f.actual, json!("Alice"));
    assert!(f.message.contains("$.extra"), "{}", f.message);
}

#[test]
fn snapshots_create_match_mismatch_update() {
    let fx = Fx::new(&[(
        "users/get-users-id.yaml",
        r#"version: 1
id: users.get
request: { method: GET, path: "/users/{id}", params: { id: 1 } }
tests:
  - name: Snapshot Test
    expect:
      ignore: ["$.createdAt"]
      body: { snapshot: true }
"#,
    )]);
    let snap = fx
        .dir
        .path()
        .join("api/__snapshots__/users.get/snapshot-test.json");

    let r = fx.run();
    assert_pass(&r, "users.get", "Snapshot Test");
    assert!(snap.is_file(), "snapshot should be created");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&snap).unwrap()).unwrap();
    assert_eq!(stored["createdAt"], "<ignored>");
    assert_eq!(stored["name"], "Alice");

    assert_pass(&fx.run(), "users.get", "Snapshot Test");

    let mut changed = stored.clone();
    changed["name"] = json!("Zed");
    std::fs::write(&snap, serde_json::to_string_pretty(&changed).unwrap()).unwrap();
    let r = fx.run();
    let t = find(&r, "users.get", "Snapshot Test");
    assert_eq!(t.status, TestStatus::Failed);
    assert_eq!(t.failures[0].check, "body.snapshot");
    assert_eq!(t.failures[0].path.as_deref(), Some("$.name"));
    assert_eq!(t.failures[0].expected, json!("Zed"));
    assert_eq!(t.failures[0].actual, json!("Alice"));

    let r = fx.run_with(TestOptions {
        update_snapshots: true,
        ..Default::default()
    });
    assert_pass(&r, "users.get", "Snapshot Test");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&snap).unwrap()).unwrap();
    assert_eq!(stored["name"], "Alice");
    assert_pass(&fx.run(), "users.get", "Snapshot Test");
}

#[test]
fn smoke_skip_only_env_and_filter() {
    let fx = Fx::new(&[
        (
            "a/get-items.yaml",
            "version: 1\nid: a.items\nrequest: { method: GET, path: /items }\n",
        ),
        (
            "a/get-boom.yaml",
            "version: 1\nid: a.boom\nrequest: { method: GET, path: /boom }\n",
        ),
        (
            "a/get-user.yaml",
            "version: 1\nid: a.user\nrequest: { method: GET, path: \"/users/{id}\" }\n",
        ),
        (
            "b/get-text.yaml",
            r#"version: 1
id: b.text
request: { method: GET, path: /text }
tests:
  - name: skipped on purpose
    skip: true
  - name: staging only
    only_env: [staging]
    expect: { status: 200 }
  - name: local only
    only_env: [local]
    expect:
      status: 200
      headers: { content-type: "/^text\\/plain/" }
      body: { contains: world }
  - name: text mismatch
    expect:
      body: { contains: galaxy }
"#,
        ),
    ]);
    let r = fx.run();
    assert_pass(&r, "a.items", "smoke");
    let boom = find(&r, "a.boom", "smoke");
    assert_eq!(boom.status, TestStatus::Failed);
    assert!(
        boom.failures[0]
            .message
            .contains("no tests defined; smoke check only — add tests to api/a/get-boom.yaml"),
        "{}",
        boom.failures[0].message
    );
    let user = find(&r, "a.user", "smoke");
    assert_eq!(user.status, TestStatus::Skipped);
    assert!(user.error.as_ref().unwrap().contains("id"));
    assert_eq!(
        find(&r, "b.text", "skipped on purpose").status,
        TestStatus::Skipped
    );
    assert_eq!(
        find(&r, "b.text", "staging only").status,
        TestStatus::Skipped
    );
    assert_pass(&r, "b.text", "local only");
    let mism = find(&r, "b.text", "text mismatch");
    assert_eq!(mism.failures[0].check, "body.contains");
    assert!(!r.ok);

    let r = fx.run_with(TestOptions {
        env: Some("staging".into()),
        ..Default::default()
    });
    assert_pass(&r, "b.text", "staging only");
    assert_eq!(find(&r, "b.text", "local only").status, TestStatus::Skipped);

    // filter: by id, by file, by test name (case-insensitive)
    let r = fx.run_with(TestOptions {
        filter: Some("A.ITEMS".into()),
        ..Default::default()
    });
    assert_eq!(r.results.len(), 1);
    let r = fx.run_with(TestOptions {
        filter: Some("b/get-text".into()),
        ..Default::default()
    });
    assert_eq!(r.results.len(), 4);
    let r = fx.run_with(TestOptions {
        filter: Some("Local Only".into()),
        ..Default::default()
    });
    assert_eq!(r.results.len(), 1);
    assert_eq!(r.results[0].test, "local only");

    // bail stops at first failure
    let r = fx.run_with(TestOptions {
        bail: true,
        ..Default::default()
    });
    assert_eq!(r.results.last().unwrap().status, TestStatus::Failed);
    assert_eq!(r.summary.failed, 1);
    assert!(r.results.len() < 8);
}

#[test]
fn errors_undefined_variable_connection_refused_and_capture() {
    let fx = Fx::new(&[(
        "x/get-items.yaml",
        r#"version: 1
id: x.items
request: { method: GET, path: /items }
tests:
  - name: undefined header var
    request:
      headers: { X-Thing: "{{ nope }}" }
    expect: { status: 200 }
  - name: undefined env var
    request:
      query: { k: "{{ env.DEFINITELY_NOT_SET_ZZ }}" }
  - name: capture missing path
    expect: { status: 200 }
    capture: { first: "$.items[5].id" }
  - name: capture header and status
    expect: { status: 200 }
    capture: { ct: "header:Content-Type", code: status }
"#,
    )]);
    let r = fx.run();
    assert!(r.load_errors.is_empty(), "{:?}", r.load_errors);
    let t = find(&r, "x.items", "undefined header var");
    assert_eq!(t.status, TestStatus::Error);
    let msg = t.error.as_ref().unwrap();
    assert!(msg.contains("nope") && msg.contains("variables"), "{}", msg);
    let t = find(&r, "x.items", "undefined env var");
    assert!(t.error.as_ref().unwrap().contains(".env"));
    let t = find(&r, "x.items", "capture missing path");
    assert_eq!(t.status, TestStatus::Failed);
    assert_eq!(t.failures[0].check, "capture");
    assert_pass(&r, "x.items", "capture header and status");
    assert_eq!(r.summary.errors, 2);

    let r = fx.run_with(TestOptions {
        base_url: Some("http://127.0.0.1:1".into()),
        filter: Some("capture header".into()),
        ..Default::default()
    });
    let t = &r.results[0];
    assert_eq!(t.status, TestStatus::Error);
    assert!(
        t.error
            .as_ref()
            .unwrap()
            .contains("Is the server running at http://127.0.0.1:1?"),
        "{:?}",
        t.error
    );
    assert!(!r.ok);
}

#[test]
fn config_errors_unknown_env_and_cycle() {
    let fx = Fx::new(&[
        (
            "a/get-a.yaml",
            "version: 1\nid: a\ndepends_on: [b]\nrequest: { method: GET, path: /items }\n",
        ),
        (
            "a/get-b.yaml",
            "version: 1\nid: b\ndepends_on: [a]\nrequest: { method: GET, path: /items }\n",
        ),
    ]);
    let e = run_tests(
        &fx.project(),
        &TestOptions {
            env: Some("prod".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    let msg = format!("{:#}", e);
    assert!(
        msg.contains("prod") && msg.contains("local") && msg.contains("staging"),
        "{}",
        msg
    );
    let e = run_tests(&fx.project(), &TestOptions::default()).unwrap_err();
    assert!(format!("{:#}", e).contains("cycle"));
}

#[test]
fn redirect_slow_request_building_and_logs() {
    let fx = Fx::new(&[
        (
            "r/get-redirect.yaml",
            r#"version: 1
id: r.redirect
request: { method: GET, path: /redirect }
tests:
  - name: not followed
    expect:
      status: 302
      headers: { location: "/users/1" }
  - name: slow
    request: { path: /slow }
    expect:
      status: 200
      max_ms: 50
"#,
        ),
        (
            "r/post-echo.yaml",
            r#"version: 1
id: r.echo
request:
  method: POST
  path: /echo
  query: { a: 1, tags: [x, "y z"], drop: me, none: null }
  headers: { X-Internal: topsecretvalue }
  body: { keep: 1, nested: { a: 1, b: 2 } }
tests:
  - name: merge
    request:
      query: { drop: null, flag: true }
      body: { nested: { b: null, c: "{{ greeting }}" } }
    expect:
      status: 200
      body:
        match:
          $.method: POST
          $.contentType: application/json
          $.xenv: local
          $.json: { keep: 1, nested: { a: 1, c: hello } }
  - name: form
    request:
      headers: { Content-Type: application/x-www-form-urlencoded }
      body: { keep: null, nested: null, a: "b c", n: 2 }
    expect:
      body:
        match:
          $.body: a=b%20c&n=2
  - name: raw string body
    request:
      headers: { Content-Type: text/plain }
      body: "just text"
    expect:
      body: { match: { $.body: just text } }
  - name: leak masked
    request: { method: GET, path: /leak }
    expect:
      body: { match: { $.ok: false } }
"#,
        ),
    ]);
    let r = fx.run();
    assert_pass(&r, "r.redirect", "not followed");
    let slow = find(&r, "r.redirect", "slow");
    assert_eq!(slow.failures[0].check, "max_ms");
    assert!(slow.duration_ms >= 250);
    assert_pass(&r, "r.echo", "merge");
    let echo = find(&r, "r.echo", "merge");
    let q = echo.response.as_ref().unwrap().body.as_ref().unwrap()["query"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(q, "a=1&tags=x&tags=y%20z&flag=true");
    assert_eq!(echo.request.as_ref().unwrap().headers["X-Internal"], "***");
    assert_pass(&r, "r.echo", "form");
    assert_pass(&r, "r.echo", "raw string body");
    let leak = find(&r, "r.echo", "leak masked");
    assert_eq!(leak.status, TestStatus::Failed);
    let ser = serde_json::to_string(leak).unwrap();
    assert!(!ser.contains(SECRET));
    assert!(ser.contains("pw=***"));
}

fn well_formed(xml: &str) {
    let tag = regex::Regex::new(r"<(/?)([A-Za-z_][\w.-]*)([^<>]*?)(/?)>").unwrap();
    let mut stack: Vec<String> = Vec::new();
    let mut tags = 0;
    for c in tag.captures_iter(xml) {
        tags += 1;
        let (close, name, selfc) = (&c[1] == "/", c[2].to_string(), &c[4] == "/");
        if close {
            assert_eq!(
                stack.pop().as_deref(),
                Some(name.as_str()),
                "mismatched </{}>",
                name
            );
        } else if !selfc {
            stack.push(name);
        }
    }
    assert!(stack.is_empty(), "unclosed {:?}", stack);
    let lt = xml.matches('<').count();
    assert_eq!(lt, tags + 1, "stray `<` (declaration counted once)");
    assert!(!xml.contains("& "), "unescaped ampersand");
    let amp = regex::Regex::new(r"&(?:amp|lt|gt|quot|apos);").unwrap();
    assert_eq!(
        xml.matches('&').count(),
        amp.find_iter(xml).count(),
        "bad entity"
    );
}

#[test]
fn cli_exit_codes_and_junit() {
    let fx = Fx::new(&[(
        "j/get-items.yaml",
        r#"version: 1
id: j.items
request: { method: GET, path: /items }
tests:
  - name: ok <fine> & "quoted"
    expect: { status: 200 }
  - name: fails
    expect:
      status: 404
      body: { match: { $.total: "a<b&c" } }
  - name: errors
    request: { headers: { X: "{{ undefinedvar }}" } }
  - name: skipped
    skip: true
"#,
    )]);
    let junit = fx.dir.path().join("out/junit.xml");
    let args = TestArgs {
        filter: None,
        env: None,
        base_url: None,
        json: true,
        junit: Some(junit.clone()),
        update_snapshots: false,
        bail: false,
        timeout: Some(5000),
        verbose: false,
    };
    assert_eq!(run_cli(fx.dir.path(), &args).unwrap(), 1);
    let xml = std::fs::read_to_string(&junit).unwrap();
    well_formed(&xml);
    assert!(xml.contains("<testsuite name=\"api/j/get-items.yaml\" tests=\"4\" failures=\"1\" errors=\"1\" skipped=\"1\""), "{}", xml);
    assert!(xml.contains("<failure type=\"status\""));
    assert!(xml.contains("<error message="));
    assert!(xml.contains("<skipped"));
    assert!(xml.contains("&lt;fine&gt; &amp; &quot;quoted&quot;"));

    let args_ok = TestArgs {
        filter: Some("ok <fine>".into()),
        junit: None,
        json: false,
        verbose: true,
        ..args
    };
    assert_eq!(run_cli(fx.dir.path(), &args_ok).unwrap(), 0);

    let args_bad_env = TestArgs {
        filter: None,
        env: Some("nope".into()),
        json: false,
        junit: None,
        bail: false,
        update_snapshots: false,
        base_url: None,
        timeout: None,
        verbose: false,
    };
    assert!(run_cli(fx.dir.path(), &args_bad_env).is_err());
}

#[test]
fn load_errors_make_report_not_ok() {
    let fx = Fx::new(&[
        (
            "a/get-items.yaml",
            "version: 1\nid: a.items\nrequest: { method: GET, path: /items }\n",
        ),
        ("a/broken.yaml", "version: 1\nid: [oops\n"),
    ]);
    let r = fx.run();
    assert_eq!(r.load_errors.len(), 1);
    assert!(!r.ok);
    assert_eq!(r.summary.passed, 1);
}
