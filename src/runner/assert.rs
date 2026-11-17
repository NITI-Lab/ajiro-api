//! Assertions: status, headers, timing and body checks. All failures of a test
//! are collected (no early exit).

use super::http::Response;
use super::jsonpath::{self, json_eq, Diff, Op};
use super::redact::Redactor;
use crate::model::{Expect, SchemaRef};
use crate::report::Failure;
use regex::RegexBuilder;

/// Maximum compiled size for user-provided regex patterns (ReDoS mitigation).
const REGEX_SIZE_LIMIT: usize = 1_000_000;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Context<'a> {
    /// Expectations with all templates already rendered.
    pub expect: &'a Expect,
    pub resp: &'a Response,
    pub redactor: &'a Redactor,
    /// Directory of the endpoint YAML file (schema paths are relative to it).
    pub endpoint_dir: &'a Path,
    pub api_dir: &'a Path,
    pub snapshot_path: &'a Path,
    pub update_snapshots: bool,
}

#[derive(Default)]
pub struct Outcome {
    pub failures: Vec<Failure>,
    pub notes: Vec<String>,
}

const MAX_DIFFS_SHOWN: usize = 10;

pub fn short(v: &Value) -> String {
    let s = match v {
        Value::String(s) => format!("{:?}", s),
        other => other.to_string(),
    };
    if s.chars().count() > 120 {
        let t: String = s.chars().take(117).collect();
        format!("{}...", t)
    } else {
        s
    }
}

fn fail(
    check: &str,
    path: Option<String>,
    expected: Value,
    actual: Value,
    message: String,
) -> Failure {
    Failure {
        check: check.to_string(),
        path,
        expected,
        actual,
        message,
    }
}

pub fn check(ctx: &Context) -> Outcome {
    let mut out = Outcome::default();
    let e = ctx.expect;
    let r = ctx.resp;

    // status
    match &e.status {
        Some(s) => {
            if !s.matches(r.status) {
                let (exp, shown) = match s {
                    crate::model::StatusExpect::Code(c) => (json!(c), c.to_string()),
                    crate::model::StatusExpect::AnyOf(v) => (json!(v), format!("one of {:?}", v)),
                };
                out.failures.push(fail(
                    "status",
                    None,
                    exp,
                    json!(r.status),
                    format!("expected status {}, got {}", shown, r.status),
                ));
            }
        }
        None => {
            if r.status >= 400 {
                out.failures.push(fail(
                    "status",
                    None,
                    json!("< 400"),
                    json!(r.status),
                    format!("got status {}; expect.status is not set, so any status below 400 is accepted", r.status),
                ));
            }
        }
    }

    // headers
    for (name, expected) in &e.headers {
        let actual = r.header(name);
        let ok = match actual {
            None => false,
            Some(a) => {
                if expected.len() >= 2 && expected.starts_with('/') && expected.ends_with('/') {
                    match RegexBuilder::new(&expected[1..expected.len() - 1])
                        .size_limit(REGEX_SIZE_LIMIT)
                        .build()
                    {
                        Ok(re) => re.is_match(a),
                        Err(_) => false,
                    }
                } else {
                    a == expected
                }
            }
        };
        if !ok {
            let sens =
                ctx.redactor.is_sensitive(name) || name.eq_ignore_ascii_case("authorization");
            let (exp, act) = if sens {
                (
                    json!("***"),
                    actual.map(|_| json!("***")).unwrap_or(Value::Null),
                )
            } else {
                (
                    json!(expected),
                    actual.map(|a| json!(a)).unwrap_or(Value::Null),
                )
            };
            let msg = match actual {
                None => format!("header `{}` not present in response", name),
                Some(_) if sens => format!("header `{}` does not match", name),
                Some(a) => format!(
                    "header `{}`: expected {}, got {:?}",
                    name,
                    short(&json!(expected)),
                    a
                ),
            };
            out.failures
                .push(fail("header", Some(name.clone()), exp, act, msg));
        }
    }

    // max_ms
    if let Some(max) = e.max_ms {
        if r.elapsed_ms > max {
            out.failures.push(fail(
                "max_ms",
                None,
                json!(max),
                json!(r.elapsed_ms),
                format!("response took {} ms, limit is {} ms", r.elapsed_ms, max),
            ));
        }
    }

    // body
    if let Some(b) = &e.body {
        if let Some(expected) = &b.equals {
            check_equals(ctx, expected, &mut out);
        }
        if let Some(schema) = &b.schema {
            check_schema(ctx, schema, &mut out);
        }
        for (path, expected) in &b.match_ {
            check_match(ctx, path, expected, &mut out);
        }
        if let Some(sub) = &b.contains {
            if !r.text.contains(sub.as_str()) {
                let body: String = r.text.chars().take(500).collect();
                out.failures.push(fail(
                    "body.contains",
                    None,
                    json!(sub),
                    json!(body),
                    format!("response body does not contain {}", short(&json!(sub))),
                ));
            }
        }
        if b.snapshot {
            check_snapshot(ctx, &mut out);
        }
    }
    out
}

// --------------------------------------------------------------- helpers ---

fn sanitize_diffs(diffs: Vec<Diff>, red: &Redactor) -> Vec<Diff> {
    diffs
        .into_iter()
        .map(|mut d| {
            if jsonpath::last_key(&d.path).is_some_and(|k| red.is_sensitive(&k)) {
                if !d.expected.is_null() {
                    d.expected = json!("***");
                }
                if !d.actual.is_null() {
                    d.actual = json!("***");
                }
            } else {
                d.expected = red.mask_value(&d.expected);
                d.actual = red.mask_value(&d.actual);
            }
            d
        })
        .collect()
}

fn diff_failure(check_name: &str, diffs: Vec<Diff>, red: &Redactor, what: &str) -> Failure {
    let diffs = sanitize_diffs(diffs, red);
    let total = diffs.len();
    let mut lines: Vec<String> = diffs
        .iter()
        .take(MAX_DIFFS_SHOWN)
        .map(|d| match d.note {
            "missing in actual" => format!(
                "{}: expected {}, but it is missing",
                d.path,
                short(&d.expected)
            ),
            "unexpected in actual" => format!("{}: unexpected value {}", d.path, short(&d.actual)),
            _ => format!(
                "{}: expected {}, got {}",
                d.path,
                short(&d.expected),
                short(&d.actual)
            ),
        })
        .collect();
    if total > MAX_DIFFS_SHOWN {
        lines.push(format!("... and {} more", total - MAX_DIFFS_SHOWN));
    }
    let first = &diffs[0];
    fail(
        check_name,
        Some(first.path.clone()),
        first.expected.clone(),
        first.actual.clone(),
        format!(
            "{} difference(s) from {}: {}",
            total,
            what,
            lines.join("; ")
        ),
    )
}

fn actual_json(ctx: &Context) -> Value {
    ctx.resp
        .json
        .clone()
        .unwrap_or_else(|| Value::String(ctx.resp.text.clone()))
}

fn apply_ignore(v: &mut Value, ignore: &[String], op: &Op) {
    for p in ignore {
        jsonpath::modify(v, p, op);
    }
}

// ---------------------------------------------------------------- equals ---

fn check_equals(ctx: &Context, expected: &Value, out: &mut Outcome) {
    let mut exp = expected.clone();
    let mut act = actual_json(ctx);
    apply_ignore(&mut exp, &ctx.expect.ignore, &Op::Remove);
    apply_ignore(&mut act, &ctx.expect.ignore, &Op::Remove);
    let diffs = jsonpath::diff(&exp, &act);
    if diffs.is_empty() {
        return;
    }
    out.failures.push(diff_failure(
        "body.equals",
        diffs,
        ctx.redactor,
        "the expected body",
    ));
}

// ---------------------------------------------------------------- schema ---

struct FileRetriever;

impl jsonschema::Retrieve for FileRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        if uri.scheme().as_str() != "file" {
            return Err(format!(
                "cannot fetch `{}`: only local file $refs are supported",
                uri
            )
            .into());
        }
        let path = file_uri_path_to_fs(uri.path().as_str());
        load_schema_file(Path::new(&path)).map_err(|e| e.into())
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file://` base URI for a directory. Windows paths (`C:\\x\\y`) become
/// `file:///C:/x/y/` — the drive letter must be in the path, not the host.
fn dir_to_file_uri(dir: &str) -> String {
    let mut p = percent_encode_path(dir);
    if !p.starts_with('/') {
        p.insert(0, '/');
    }
    if !p.ends_with('/') {
        p.push('/');
    }
    format!("file://{}", p)
}

/// Inverse of [`dir_to_file_uri`] for the path component of a `file:` URI:
/// `/C:/x/y.json` -> `C:/x/y.json` (Windows), `/x/y.json` unchanged.
fn file_uri_path_to_fs(path: &str) -> String {
    let decoded = percent_decode(path);
    let b = decoded.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        decoded[1..].to_string()
    } else {
        decoded
    }
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '%' => out.push_str("%25"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '\\' => out.push('/'),
            c => out.push(c),
        }
    }
    out
}

fn load_schema_file(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read schema {}: {}", path.display(), e))?;
    let is_yaml = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yaml") | Some("yml")
    );
    if is_yaml {
        serde_yaml_rust2::from_str(&text)
            .map_err(|e| format!("invalid schema {}: {}", path.display(), e))
    } else {
        serde_json::from_str(&text)
            .map_err(|e| format!("invalid JSON in schema {}: {}", path.display(), e))
    }
}

fn find_schema(ctx: &Context, p: &str) -> Option<PathBuf> {
    let cands = [
        ctx.endpoint_dir.join(p),
        ctx.api_dir.join("schemas").join(p),
        ctx.api_dir.join(p),
    ];
    cands.into_iter().find(|c| c.is_file())
}

fn check_schema(ctx: &Context, sref: &SchemaRef, out: &mut Outcome) {
    let (schema, label, base_dir): (Value, String, PathBuf) = match sref {
        SchemaRef::Inline(v) => (
            v.clone(),
            "inline".to_string(),
            ctx.endpoint_dir.to_path_buf(),
        ),
        SchemaRef::Path(p) => match find_schema(ctx, p) {
            Some(file) => match load_schema_file(&file) {
                Ok(v) => (
                    v,
                    p.clone(),
                    file.parent().unwrap_or(ctx.endpoint_dir).to_path_buf(),
                ),
                Err(msg) => {
                    out.failures
                        .push(fail("body.schema", None, json!(p), Value::Null, msg));
                    return;
                }
            },
            None => {
                out.failures.push(fail(
                    "body.schema",
                    None,
                    json!(p),
                    Value::Null,
                    format!(
                        "schema file `{}` not found (looked next to the endpoint file and in {}/schemas/)",
                        p,
                        ctx.api_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "api".into())
                    ),
                ));
                return;
            }
        },
    };
    let Some(instance) = ctx.resp.json.as_ref() else {
        out.failures.push(fail(
            "body.schema",
            Some("$".into()),
            json!({ "schema": label }),
            json!(ctx.resp.text.chars().take(200).collect::<String>()),
            "response body is not JSON, cannot validate against the schema".into(),
        ));
        return;
    };
    let base = dir_to_file_uri(&base_dir.to_string_lossy());
    let validator = match jsonschema::options()
        .with_base_uri(base)
        .with_retriever(FileRetriever)
        .build(&schema)
    {
        Ok(v) => v,
        Err(e) => {
            out.failures.push(fail(
                "body.schema",
                None,
                json!({ "schema": label }),
                Value::Null,
                format!("invalid schema `{}`: {}", label, e),
            ));
            return;
        }
    };
    for err in validator.iter_errors(instance) {
        let ptr = err.instance_path().to_string();
        let jp = jsonpath::pointer_to_jsonpath(&ptr, instance);
        let actual = instance.pointer(&ptr).cloned().unwrap_or(Value::Null);
        let sens = jsonpath::last_key(&jp).is_some_and(|k| ctx.redactor.is_sensitive(&k));
        let (actual, message) = if sens {
            (
                json!("***"),
                format!(
                    "{}: value does not satisfy the schema (schema path {})",
                    jp,
                    err.schema_path()
                ),
            )
        } else {
            (ctx.redactor.mask_value(&actual), format!("{}: {}", jp, err))
        };
        out.failures.push(fail(
            "body.schema",
            Some(jp),
            json!({ "schema": label, "rule": err.schema_path().to_string() }),
            actual,
            message,
        ));
    }
}

// ----------------------------------------------------------------- match ---

const MATCHERS: &[&str] = &[
    "$exists",
    "$type",
    "$regex",
    "$contains",
    "$len",
    "$gt",
    "$gte",
    "$lt",
    "$lte",
    "$ne",
    "$oneOf",
];

fn as_matcher(v: &Value) -> Option<(&str, &Value)> {
    match v {
        Value::Object(m) if m.len() == 1 => {
            let (k, x) = m.iter().next()?;
            MATCHERS.contains(&k.as_str()).then_some((k.as_str(), x))
        }
        _ => None,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n)
            if n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0) =>
        {
            "integer"
        }
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// `Ok(description)` when satisfied, `Err(description of expectation)` otherwise.
fn eval_matcher(op: &str, arg: &Value, actual: Option<&Value>) -> (bool, String) {
    let num = |v: &Value| v.as_f64();
    match op {
        "$exists" => {
            let want = arg.as_bool().unwrap_or(true);
            (
                actual.is_some() == want,
                if want {
                    "to exist".into()
                } else {
                    "to not exist".into()
                },
            )
        }
        "$ne" => (
            actual.map_or(true, |a| !json_eq(a, arg)),
            format!("not equal to {}", short(arg)),
        ),
        _ => {
            let Some(a) = actual else {
                return (false, describe(op, arg));
            };
            let ok = match op {
                "$type" => match arg.as_str() {
                    Some("number") => matches!(a, Value::Number(_)),
                    Some(t) => type_name(a) == t || (t == "integer" && type_name(a) == "integer"),
                    None => false,
                },
                "$regex" => match (arg.as_str(), a.as_str()) {
                    (Some(p), Some(s)) => RegexBuilder::new(p)
                        .size_limit(REGEX_SIZE_LIMIT)
                        .build()
                        .map(|re| re.is_match(s))
                        .unwrap_or(false),
                    _ => false,
                },
                "$contains" => match a {
                    Value::String(s) => arg.as_str().is_some_and(|sub| s.contains(sub)),
                    Value::Array(items) => items.iter().any(|i| json_eq(i, arg)),
                    Value::Object(m) => arg.as_str().is_some_and(|k| m.contains_key(k)),
                    _ => false,
                },
                "$len" => {
                    let len = match a {
                        Value::String(s) => Some(s.chars().count()),
                        Value::Array(x) => Some(x.len()),
                        Value::Object(m) => Some(m.len()),
                        _ => None,
                    };
                    match (len, as_matcher(arg)) {
                        // `$len: { $gte: 1 }`
                        (Some(l), Some((op2, arg2))) => {
                            eval_matcher(op2, arg2, Some(&Value::from(l as u64))).0
                        }
                        (Some(l), None) => arg.as_u64() == Some(l as u64),
                        (None, _) => false,
                    }
                }
                "$gt" => matches!((a.as_f64(), num(arg)), (Some(x), Some(y)) if x > y),
                "$gte" => matches!((a.as_f64(), num(arg)), (Some(x), Some(y)) if x >= y),
                "$lt" => matches!((a.as_f64(), num(arg)), (Some(x), Some(y)) if x < y),
                "$lte" => matches!((a.as_f64(), num(arg)), (Some(x), Some(y)) if x <= y),
                "$oneOf" => arg
                    .as_array()
                    .is_some_and(|items| items.iter().any(|i| json_eq(i, a))),
                _ => false,
            };
            (ok, describe(op, arg))
        }
    }
}

fn describe(op: &str, arg: &Value) -> String {
    match op {
        "$type" => format!("type {}", short(arg)),
        "$regex" => format!("to match regex {}", short(arg)),
        "$contains" => format!("to contain {}", short(arg)),
        "$len" => format!("length {}", short(arg)),
        "$gt" => format!("> {}", short(arg)),
        "$gte" => format!(">= {}", short(arg)),
        "$lt" => format!("< {}", short(arg)),
        "$lte" => format!("<= {}", short(arg)),
        "$oneOf" => format!("one of {}", short(arg)),
        "$ne" => format!("not equal to {}", short(arg)),
        _ => format!("{} {}", op, short(arg)),
    }
}

fn check_match(ctx: &Context, path: &str, expected: &Value, out: &mut Outcome) {
    let Some(json) = ctx.resp.json.as_ref() else {
        out.failures.push(fail(
            "body.match",
            Some(path.to_string()),
            expected.clone(),
            Value::Null,
            format!("{}: response body is not JSON", path),
        ));
        return;
    };
    // A path that can select several nodes (`$.items[*].id`, filters, `..`)
    // is compared as the array of ALL matches, so `$contains`, `$len` and
    // equality see every element. For `$exists`, an empty result = absent.
    let multi_holder: Option<Value> = if jsonpath::is_multi(path) {
        match jsonpath::query_all(json, path) {
            Ok(all) => Some(Value::Array(all)),
            Err(msg) => {
                out.failures.push(fail(
                    "body.match",
                    Some(path.to_string()),
                    expected.clone(),
                    Value::Null,
                    msg,
                ));
                return;
            }
        }
    } else {
        None
    };
    let looked_up = match &multi_holder {
        Some(Value::Array(all))
            if all.is_empty() && matches!(as_matcher(expected), Some(("$exists", _))) =>
        {
            Ok(None)
        }
        Some(v) => Ok(Some(v)),
        None => jsonpath::query_first(json, path),
    };
    let actual = match looked_up {
        Ok(a) => a,
        Err(msg) => {
            out.failures.push(fail(
                "body.match",
                Some(path.to_string()),
                expected.clone(),
                Value::Null,
                msg,
            ));
            return;
        }
    };
    let sens = jsonpath::last_key(path).is_some_and(|k| ctx.redactor.is_sensitive(&k));
    let (ok, wanted) = match as_matcher(expected) {
        Some((op, arg)) => eval_matcher(op, arg, actual),
        None => (
            actual.is_some_and(|a| json_eq(a, expected)),
            format!("equal to {}", short(expected)),
        ),
    };
    if ok {
        return;
    }
    let (exp_v, act_v) = if sens {
        (
            json!("***"),
            actual.map(|_| json!("***")).unwrap_or(Value::Null),
        )
    } else {
        (
            ctx.redactor.mask_value(expected),
            actual
                .map(|a| ctx.redactor.mask_value(a))
                .unwrap_or(Value::Null),
        )
    };
    let message = match (actual, sens) {
        (None, _) => format!("{}: path not found (expected {})", path, wanted),
        (Some(_), true) => format!("{}: value does not match the expectation", path),
        (Some(a), false) => format!(
            "{}: expected {}, got {}",
            path,
            wanted,
            short(&ctx.redactor.mask_value(a))
        ),
    };
    out.failures.push(fail(
        "body.match",
        Some(path.to_string()),
        exp_v,
        act_v,
        message,
    ));
}

// -------------------------------------------------------------- snapshot ---

fn check_snapshot(ctx: &Context, out: &mut Outcome) {
    let mut actual = actual_json(ctx);
    apply_ignore(
        &mut actual,
        &ctx.expect.ignore,
        &Op::Replace(json!("<ignored>")),
    );
    let actual = ctx.redactor.mask_value(&actual);
    let path = ctx.snapshot_path;
    let write = |out: &mut Outcome, verb: &str| {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut text = serde_json::to_string_pretty(&actual).unwrap_or_default();
        text.push('\n');
        match std::fs::write(path, text) {
            Ok(()) => out
                .notes
                .push(format!("snapshot {}: {}", verb, path.display())),
            Err(e) => out.failures.push(fail(
                "body.snapshot",
                None,
                Value::Null,
                Value::Null,
                format!("cannot write snapshot {}: {}", path.display(), e),
            )),
        }
    };
    if ctx.update_snapshots {
        write(out, "updated");
        return;
    }
    if !path.exists() {
        write(out, "written");
        return;
    }
    let stored: Value = match std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            out.failures.push(fail(
                "body.snapshot",
                None,
                Value::Null,
                Value::Null,
                format!(
                    "cannot read snapshot {}: {} (re-create it with --update-snapshots)",
                    path.display(),
                    e
                ),
            ));
            return;
        }
    };
    let diffs = jsonpath::diff(&stored, &actual);
    if !diffs.is_empty() {
        let mut f = diff_failure("body.snapshot", diffs, ctx.redactor, "the stored snapshot");
        f.message
            .push_str(" (if the change is intended, run with --update-snapshots)");
        out.failures.push(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matchers() {
        let a = json!(5);
        assert!(eval_matcher("$gt", &json!(3), Some(&a)).0);
        assert!(!eval_matcher("$lt", &json!(3), Some(&a)).0);
        assert!(eval_matcher("$type", &json!("integer"), Some(&a)).0);
        assert!(eval_matcher("$type", &json!("number"), Some(&a)).0);
        assert!(eval_matcher("$oneOf", &json!([1, 5]), Some(&a)).0);
        assert!(eval_matcher("$exists", &json!(false), None).0);
        assert!(!eval_matcher("$regex", &json!("x"), None).0);
        assert!(eval_matcher("$len", &json!(2), Some(&json!([1, 2]))).0);
        assert!(eval_matcher("$contains", &json!("ell"), Some(&json!("hello"))).0);
        assert!(eval_matcher("$contains", &json!(2), Some(&json!([1, 2]))).0);
        assert!(eval_matcher("$ne", &json!(1), Some(&json!(2))).0);
        assert!(as_matcher(&json!({"$gt": 1, "x": 2})).is_none());
    }

    #[test]
    fn percent_roundtrip() {
        assert_eq!(percent_decode(&percent_encode_path("/a b/c%d")), "/a b/c%d");
        assert_eq!(percent_decode("/a%20"), "/a ");
    }

    #[test]
    fn file_uris_work_for_unix_and_windows_paths() {
        assert_eq!(dir_to_file_uri("/tmp/a b"), "file:///tmp/a%20b/");
        assert_eq!(
            dir_to_file_uri(r"C:\Users\RUNNER~1\Temp\api\users"),
            "file:///C:/Users/RUNNER~1/Temp/api/users/"
        );
        assert_eq!(file_uri_path_to_fs("/tmp/a%20b/x.json"), "/tmp/a b/x.json");
        assert_eq!(
            file_uri_path_to_fs("/C:/Users/RUNNER~1/Temp/api/schemas/common.json"),
            "C:/Users/RUNNER~1/Temp/api/schemas/common.json"
        );
    }
}
