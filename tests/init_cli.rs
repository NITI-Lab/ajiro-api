//! `init` detection warnings, config errors surfaced by every command, and
//! JSON error objects. These run the real binary.

use std::path::Path;
use std::process::{Command, Output};

fn bin(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ajiro-api"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn init_json(dir: &Path) -> serde_json::Value {
    let out = bin(dir, &["init", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{:?}", out);
    serde_json::from_slice(&out.stdout).unwrap()
}

fn warnings(v: &serde_json::Value) -> String {
    v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn init_without_framework_warns_and_names_unsupported_stacks() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "package.json",
            r#"{"dependencies":{"@nestjs/core":"10","express":"4"}}"#,
            "NestJS",
            "",
        ),
        (
            "package.json",
            r#"{"dependencies":{"fastify":"4"}}"#,
            "Fastify",
            "",
        ),
        ("package.json", r#"{"dependencies":{"koa":"2"}}"#, "Koa", ""),
        (
            "package.json",
            r#"{"devDependencies":{"@aws-amplify/backend":"1"}}"#,
            "AWS Amplify Gen2",
            "",
        ),
        (
            "package.json",
            r#"{"devDependencies":{"@types/google-apps-script":"1"}}"#,
            "Google Apps Script",
            "",
        ),
        ("appsscript.json", "{}", "Google Apps Script", ""),
        ("requirements.txt", "Flask==3.0\n", "Flask", ""),
        (
            "pyproject.toml",
            "[project]\ndependencies = [\"django>=5\"]\n",
            "Django",
            "",
        ),
    ];
    for (file, text, name, _) in cases {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), file, text);
        let v = init_json(d.path());
        assert!(v["framework"].is_null(), "{name}: {v}");
        let w = warnings(&v);
        assert!(w.contains(name), "{name}: {w}");
        assert!(w.contains("supported: nextjs, hono"), "{w}");
        let human = bin(tempfile::tempdir().unwrap().path(), &["init"]);
        assert_eq!(human.status.code(), Some(0));
    }
}

#[test]
fn init_pages_functions_and_plain_none() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "functions/api/x.ts",
        "export const onRequest = () => {}",
    );
    write(d.path(), "wrangler.toml", "name = \"x\"\n");
    let v = init_json(d.path());
    assert!(warnings(&v).contains("Cloudflare Pages Functions"), "{v}");

    let d = tempfile::tempdir().unwrap();
    let v = init_json(d.path());
    assert!(v["framework"].is_null());
    assert!(warnings(&v).contains("no supported framework detected"));
    // Human output carries the warning too, and the exit code stays 0.
    let d = tempfile::tempdir().unwrap();
    let out = bin(d.path(), &["init"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("warning: no supported framework"));
}

#[test]
fn init_detected_framework_has_empty_warnings() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "package.json", r#"{"dependencies":{"hono":"4"}}"#);
    write(d.path(), "wrangler.toml", "main = \"src/index.ts\"\n");
    let v = init_json(d.path());
    assert_eq!(v["framework"], "hono");
    assert!(v["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn hono_client_only_spa_is_not_hono() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4","react":"19","@tanstack/react-router":"1"},"devDependencies":{"vite":"6"}}"#,
    );
    write(
        d.path(),
        "src/api.ts",
        "import { hc } from \"hono/client\";\nexport const c = hc('/');\n",
    );
    let v = init_json(d.path());
    assert!(v["framework"].is_null(), "{v}");
    let w = warnings(&v);
    assert!(w.contains("only used as a client"), "{w}");
}

#[test]
fn real_hono_servers_are_still_detected() {
    // Workers app with wrangler config.
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4"},"devDependencies":{"wrangler":"3"}}"#,
    );
    write(d.path(), "wrangler.toml", "main = \"src/index.ts\"\n");
    assert_eq!(init_json(d.path())["framework"], "hono");
    // Node server.
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4","@hono/node-server":"1"}}"#,
    );
    assert_eq!(init_json(d.path())["framework"], "hono");
    // Frontend-looking package that still defines `new Hono()`.
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "package.json",
        r#"{"dependencies":{"hono":"4","react":"19"},"devDependencies":{"vite":"6"}}"#,
    );
    write(
        d.path(),
        "src/index.tsx",
        "import { Hono } from 'hono';\nconst app = new Hono();\n",
    );
    assert_eq!(init_json(d.path())["framework"], "hono");
}

#[test]
fn invalid_config_is_an_error_for_scan_not_a_silent_default() {
    let d = tempfile::tempdir().unwrap();
    // No `version: 1`.
    write(
        d.path(),
        "ajiro-api.yaml",
        "framework: hono\nsources: [src]\n",
    );
    write(d.path(), "package.json", r#"{"dependencies":{"hono":"4"}}"#);
    for args in [
        vec!["scan"],
        vec!["list"],
        vec!["validate"],
        vec!["test"],
        vec!["openapi", "export"],
    ] {
        let out = bin(d.path(), &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {:?}", out);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("missing field `version`"), "{args:?}: {err}");
        assert!(!err.contains("could not detect"), "{err}");
    }
}

#[test]
fn json_mode_errors_are_json_on_stdout_and_text_on_stderr() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "ajiro-api.yaml", "framework: hono\n");
    for args in [
        vec!["scan", "--json"],
        vec!["list", "--json"],
        vec!["validate", "--json"],
        vec!["test", "--json"],
    ] {
        let out = bin(d.path(), &args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: {:?}", out));
        assert_eq!(v["ok"], false);
        assert_eq!(v["report_version"], 1);
        assert!(v["error"].as_str().unwrap().contains("version"), "{v}");
        assert!(String::from_utf8_lossy(&out.stderr).starts_with("error:"));
    }
    // Without --json nothing is printed on stdout.
    let out = bin(d.path(), &["list"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

#[test]
fn scan_unsupported_project_counts_code_files_and_check_fails() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "package.json",
        r#"{"dependencies":{"express":"4"}}"#,
    );
    write(d.path(), "src/a.ts", "export const a = 1;\n");
    write(d.path(), "src/logo.png", "x");
    write(d.path(), "src/logo2.png", "x");
    assert_eq!(bin(d.path(), &["init"]).status.code(), Some(0));
    let out = bin(d.path(), &["scan", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let w = v["warnings"][0].as_str().unwrap();
    assert!(w.contains("in 1 scanned source file(s)"), "{w}");
    assert!(w.contains("(supported:"), "{w}");
    let out = bin(d.path(), &["scan", "--check"]);
    assert_eq!(out.status.code(), Some(1), "{:?}", out);
    assert!(String::from_utf8_lossy(&out.stderr).contains("nothing was checked"));
}

#[test]
fn scan_without_framework_names_the_unsupported_stack() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "requirements.txt", "flask\n");
    let out = bin(d.path(), &["scan"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("could not detect the framework") && err.contains("Flask"),
        "{err}"
    );
}
