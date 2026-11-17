//! Regressions found by running ajiro-api on real projects.

use ajiro_api::config::Project;
use ajiro_api::project::load_endpoints;
use std::io::Read;
use std::process::{Command, Stdio};

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

/// `/auth/password-reset-request` and `/auth/password-reset/request` map to
/// the same file name; both must be imported.
#[test]
fn openapi_import_keeps_paths_whose_file_names_collide() {
    let dir = temp_project(&[]);
    let project = Project::discover(dir.path()).unwrap();
    let doc = r#"{
      "openapi": "3.1.0",
      "info": { "title": "t", "version": "1" },
      "paths": {
        "/api/auth/password-reset-request": { "post": { "responses": { "204": { "description": "ok" } } } },
        "/api/auth/password-reset/request": { "post": { "responses": { "204": { "description": "ok" } } } }
      }
    }"#;
    let report = ajiro_api::openapi::import_str(&project, doc, false).unwrap();
    assert_eq!(report.created.len(), 2, "{:?}", report);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let (eps, errs) = load_endpoints(&project);
    assert!(errs.is_empty(), "{:?}", errs);
    let mut paths: Vec<_> = eps.iter().map(|e| e.def.request.path.clone()).collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "/api/auth/password-reset-request",
            "/api/auth/password-reset/request"
        ]
    );
    let mut files: Vec<_> = report.created.clone();
    files.sort();
    assert_eq!(
        files,
        [
            "api/auth/post-api-auth-password-reset-request-2.yaml",
            "api/auth/post-api-auth-password-reset-request.yaml"
        ]
    );
}

/// `ajiro-api list | head -1` must not panic when the reader goes away.
#[test]
fn closed_stdout_exits_quietly() {
    let mut files = Vec::new();
    for i in 0..3000 {
        files.push((
            format!("api/r/get-r-e{i}.yaml"),
            format!(
                "version: 1\nid: r.e{i}\nrequest: {{ method: GET, path: /r/e{i} }}\ntests: []\n"
            ),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let dir = temp_project(&refs);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ajiro-api"))
        .args(["list"])
        .current_dir(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // read one byte, then close our end of the pipe
    let mut stdout = child.stdout.take().unwrap();
    let mut one = [0u8; 1];
    stdout.read_exact(&mut one).unwrap();
    drop(stdout);
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    let status = child.wait().unwrap();
    assert!(!err.contains("panicked"), "stderr: {err}");
    assert_eq!(status.code(), Some(0), "stderr: {err}");
}

type PortCase<'a> = (&'a [(&'a str, &'a str)], Option<(u16, &'a str)>);

#[test]
fn init_detects_the_dev_server_port() {
    let cases: &[PortCase] = &[
        (
            &[("src/index.ts", "serve({ fetch: app.fetch, port: 8000 });\n")],
            Some((8000, "src/index.ts")),
        ),
        (
            &[(
                "src/index.ts",
                "const port = Number(process.env.PORT) || 8787\n",
            )],
            Some((8787, "src/index.ts")),
        ),
        (
            &[
                (".env.example", "DATABASE_URL=x\nPORT=8000\n"),
                ("src/index.ts", "serve({ port: 3000 })"),
            ],
            Some((8000, ".env.example")),
        ),
        (
            &[("package.json", r#"{"scripts":{"dev":"next dev -p 4000"}}"#)],
            Some((4000, "package.json")),
        ),
        (
            &[("main.go", "http.ListenAndServe(\":9090\", mux)\n")],
            Some((9090, "main.go")),
        ),
        (
            &[(
                "app/main.py",
                "uvicorn.run(app, host=\"0.0.0.0\", port=8001)\n",
            )],
            Some((8001, "app/main.py")),
        ),
        (&[("src/routes.ts", "app.get('/x', h)\n")], None),
    ];
    for (files, want) in cases {
        let dir = temp_project(files);
        let got = ajiro_api::init::detect_port(dir.path());
        let got = got.as_ref().map(|(p, f)| (*p, f.as_str()));
        assert_eq!(got, *want, "files: {:?}", files);
    }
}
