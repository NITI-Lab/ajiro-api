//! OpenAPI 3.x import/export. (OWNER: integrations agent)

use crate::cli::OpenapiCommand;
use crate::config::Project;
use crate::importers::{print_report, write_endpoints, ImportReport};
use crate::model::{
    EndpointDef, MockResponse, MockSpec, RequestSpec, SchemaRef, StatusExpect, TestCase,
    SCHEMA_VERSION,
};
use crate::naming;
use crate::project::{load_endpoints, LoadedEndpoint};
use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use std::path::Path;

pub fn run_cli(dir: &Path, cmd: &OpenapiCommand) -> Result<i32> {
    let project = Project::discover_or_default(dir)?;
    match cmd {
        OpenapiCommand::Export { out, title } => {
            let doc = export(&project, title.as_deref());
            let yaml = out
                .as_ref()
                .and_then(|p| p.extension().and_then(|e| e.to_str()))
                .map(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"))
                .unwrap_or(false);
            let text = if yaml {
                serde_yaml::to_string(&doc)?
            } else {
                serde_json::to_string_pretty(&doc)? + "\n"
            };
            match out {
                Some(p) => {
                    if let Some(parent) = p.parent() {
                        if !parent.as_os_str().is_empty() {
                            std::fs::create_dir_all(parent)?;
                        }
                    }
                    std::fs::write(p, text)
                        .with_context(|| format!("cannot write {}", p.display()))?;
                    eprintln!("wrote {}", p.display());
                }
                None => print!("{}", text),
            }
            Ok(crate::exit::OK)
        }
        OpenapiCommand::Import {
            file,
            overwrite,
            json,
        } => {
            let text = std::fs::read_to_string(file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let report = import_str(&project, &text, *overwrite)?;
            print_report(&report, *json)?;
            Ok(crate::exit::OK)
        }
    }
}

// ---------------------------------------------------------------- export ---

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Response",
    }
}

/// Infer a (loose) JSON Schema from an example value.
pub fn infer_schema(v: &Value) -> Value {
    match v {
        Value::Null => json!({ "type": "null" }),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                json!({ "type": "integer" })
            } else {
                json!({ "type": "number" })
            }
        }
        Value::String(_) => json!({ "type": "string" }),
        Value::Array(a) => match a.first() {
            Some(f) => json!({ "type": "array", "items": infer_schema(f) }),
            None => json!({ "type": "array", "items": {} }),
        },
        Value::Object(o) => {
            let props: Map<String, Value> = o
                .iter()
                .map(|(k, v)| (k.clone(), infer_schema(v)))
                .collect();
            let required: Vec<Value> = o.keys().map(|k| Value::String(k.clone())).collect();
            let mut m = Map::new();
            m.insert("type".into(), json!("object"));
            m.insert("properties".into(), Value::Object(props));
            if !required.is_empty() {
                m.insert("required".into(), Value::Array(required));
            }
            Value::Object(m)
        }
    }
}

fn is_template(s: &str) -> bool {
    s.contains("{{")
}

fn example_of(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) if is_template(s) => None,
        Value::Null => None,
        other => Some(other.clone()),
    }
}

const SKIP_HEADERS: &[&str] = &[
    "authorization",
    "content-type",
    "accept",
    "user-agent",
    "cookie",
    "host",
    "content-length",
    "accept-encoding",
    "connection",
];

fn read_schema_file(ep: &LoadedEndpoint, r: &SchemaRef) -> Option<Value> {
    match r {
        SchemaRef::Inline(v) => Some(v.clone()),
        SchemaRef::Path(p) => {
            let base = ep.path.parent()?;
            let text = std::fs::read_to_string(base.join(p)).ok()?;
            let v: Value = serde_json::from_str(&text)
                .or_else(|_| serde_yaml::from_str(&text))
                .ok()?;
            // strip `$schema`/`$id` meta keys that are noise inside an OpenAPI document
            match v {
                Value::Object(mut o) => {
                    o.remove("$schema");
                    o.remove("$id");
                    Some(Value::Object(o))
                }
                other => Some(other),
            }
        }
    }
}

fn operation_for(ep: &LoadedEndpoint, has_bearer: &mut bool) -> Value {
    let def = &ep.def;
    let mut op = Map::new();
    op.insert("operationId".into(), json!(def.id));
    if let Some(s) = &def.summary {
        op.insert("summary".into(), json!(s));
    }
    if let Some(d) = &def.description {
        op.insert("description".into(), json!(d));
    }
    let tags: Vec<String> = if def.tags.is_empty() {
        let res = ep
            .rel_path
            .split('/')
            .rev()
            .nth(1)
            .map(str::to_string)
            .unwrap_or_else(|| naming::resource_of(&def.request.path));
        vec![res]
    } else {
        def.tags.clone()
    };
    op.insert("tags".into(), json!(tags));

    // parameters
    let mut params: Vec<Value> = Vec::new();
    for name in naming::path_params(&def.request.path) {
        let mut p =
            json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } });
        if let Some(ex) = def.request.params.get(&name).and_then(example_of) {
            p["example"] = ex;
        }
        params.push(p);
    }
    for (k, v) in &def.request.query {
        let schema = match v {
            Value::Null => json!({ "type": "string" }),
            Value::String(s) if is_template(s) => json!({ "type": "string" }),
            other => infer_schema(other),
        };
        let mut p = json!({ "name": k, "in": "query", "schema": schema });
        if let Some(ex) = example_of(v) {
            p["example"] = ex;
        }
        params.push(p);
    }
    for (k, v) in &def.request.headers {
        let lk = k.to_ascii_lowercase();
        if lk == "authorization" {
            if v.trim().to_ascii_lowercase().starts_with("bearer") {
                *has_bearer = true;
            }
            continue;
        }
        if SKIP_HEADERS.contains(&lk.as_str()) {
            continue;
        }
        let mut p = json!({ "name": k, "in": "header", "schema": { "type": "string" } });
        if !is_template(v) && !v.is_empty() {
            p["example"] = json!(v);
        }
        params.push(p);
    }
    if !params.is_empty() {
        op.insert("parameters".into(), Value::Array(params));
    }

    // request body
    if let Some(body) = &def.request.body {
        let ct = def
            .request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.split(';').next().unwrap_or("").trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "application/json".into());
        let media = if body.is_string() {
            json!({ "schema": { "type": "string" }, "example": body })
        } else {
            json!({ "schema": infer_schema(body), "example": body })
        };
        op.insert("requestBody".into(), json!({ "content": { ct: media } }));
    }

    // responses
    let mut responses: IndexMap<u16, Map<String, Value>> = IndexMap::new();
    let ensure = |m: &mut IndexMap<u16, Map<String, Value>>, code: u16| {
        m.entry(code).or_insert_with(|| {
            let mut r = Map::new();
            r.insert("description".into(), json!(reason(code)));
            r
        });
    };
    let put_media = |code: u16,
                     schema: Option<Value>,
                     example: Option<Value>,
                     responses: &mut IndexMap<u16, Map<String, Value>>| {
        ensure(responses, code);
        let r = responses.get_mut(&code).unwrap();
        let content = r
            .entry("content")
            .or_insert_with(|| json!({ "application/json": {} }));
        let media = &mut content["application/json"];
        if let Some(s) = schema {
            media["schema"] = s;
        }
        if let Some(e) = example {
            if media.get("schema").is_none() {
                media["schema"] = infer_schema(&e);
            }
            media["example"] = e;
        }
    };
    if let Some(mock) = &def.mock {
        let all = mock.default.iter().chain(mock.scenarios.values());
        for r in all {
            match &r.body {
                Some(b) => put_media(r.status, None, Some(b.clone()), &mut responses),
                None => ensure(&mut responses, r.status),
            }
        }
    }
    for t in &def.tests {
        let codes: Vec<u16> = match &t.expect.status {
            Some(StatusExpect::Code(c)) => vec![*c],
            Some(StatusExpect::AnyOf(v)) => v.clone(),
            None => vec![],
        };
        for c in &codes {
            ensure(&mut responses, *c);
        }
        if let (Some(b), Some(first)) = (&t.expect.body, codes.first()) {
            if let Some(sr) = &b.schema {
                if let Some(schema) = read_schema_file(ep, sr) {
                    put_media(*first, Some(schema), None, &mut responses);
                }
            }
        }
    }
    if responses.is_empty() {
        ensure(&mut responses, 200);
    }
    let mut codes: Vec<u16> = responses.keys().copied().collect();
    codes.sort_unstable();
    let mut rmap = Map::new();
    for c in codes {
        rmap.insert(
            c.to_string(),
            Value::Object(responses.shift_remove(&c).unwrap()),
        );
    }
    op.insert("responses".into(), Value::Object(rmap));

    if *has_bearer {
        op.insert("security".into(), json!([{ "bearerAuth": [] }]));
    }
    op.insert("x-ajiro-file".into(), json!(ep.rel_path));
    if let Some(src) = &def.source {
        op.insert(
            "x-ajiro-source".into(),
            json!({ "file": src.file, "line": src.line }),
        );
    }
    Value::Object(op)
}

/// Build the OpenAPI 3.1 document for the project.
pub fn export(project: &Project, title: Option<&str>) -> Value {
    let (eps, _errs) = load_endpoints(project);
    let title = title.map(str::to_string).unwrap_or_else(|| {
        project
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "API".into())
    });
    let servers: Vec<Value> = project
        .config
        .environments
        .iter()
        .map(|(n, e)| json!({ "url": e.base_url, "description": n }))
        .collect();

    let mut paths: Map<String, Value> = Map::new();
    let mut any_bearer = false;
    for ep in &eps {
        let mut bearer = false;
        let op = operation_for(ep, &mut bearer);
        any_bearer |= bearer;
        let method = ep.def.request.method.to_ascii_lowercase();
        let path = ep.def.request.path.clone();
        let item = paths.entry(path).or_insert_with(|| json!({}));
        item[method] = op;
    }
    // stable order: by path
    let mut sorted: Vec<(String, Value)> = paths.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let paths: Map<String, Value> = sorted.into_iter().collect();

    let mut doc = Map::new();
    doc.insert("openapi".into(), json!("3.1.0"));
    doc.insert("info".into(), json!({ "title": title, "version": "0.0.0" }));
    if !servers.is_empty() {
        doc.insert("servers".into(), Value::Array(servers));
    }
    doc.insert("paths".into(), Value::Object(paths));
    if any_bearer {
        doc.insert(
            "components".into(),
            json!({ "securitySchemes": { "bearerAuth": { "type": "http", "scheme": "bearer" } } }),
        );
    }
    Value::Object(doc)
}

// ---------------------------------------------------------------- import ---

fn resolve_ref<'a>(doc: &'a Value, r: &str) -> Option<&'a Value> {
    let p = r.strip_prefix('#')?;
    doc.pointer(p)
}

/// Resolve `$ref` chains (depth limited).
fn resolve<'a>(doc: &'a Value, v: &'a Value) -> &'a Value {
    let mut cur = v;
    for _ in 0..16 {
        match cur
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| resolve_ref(doc, r))
        {
            Some(n) => cur = n,
            None => break,
        }
    }
    cur
}

/// Generate an example value from a JSON Schema.
pub fn example_from_schema(doc: &Value, schema: &Value, depth: usize) -> Value {
    if depth > 6 {
        return Value::Null;
    }
    let s = resolve(doc, schema);
    if let Some(e) = s.get("example") {
        return e.clone();
    }
    if let Some(e) = s
        .get("examples")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        return e.clone();
    }
    if let Some(d) = s.get("default") {
        return d.clone();
    }
    if let Some(e) = s
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        return e.clone();
    }
    if let Some(c) = s.get("const") {
        return c.clone();
    }
    if let Some(all) = s.get("allOf").and_then(Value::as_array) {
        let mut merged = Map::new();
        let mut other = None;
        for part in all {
            match example_from_schema(doc, part, depth + 1) {
                Value::Object(o) => merged.extend(o),
                v => other = Some(v),
            }
        }
        if let Some(Value::Object(o)) = s.get("properties").map(|_| example_object(doc, s, depth)) {
            merged.extend(o);
        }
        return if merged.is_empty() {
            other.unwrap_or(Value::Null)
        } else {
            Value::Object(merged)
        };
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(first) = s.get(key).and_then(Value::as_array).and_then(|a| a.first()) {
            return example_from_schema(doc, first, depth + 1);
        }
    }
    let ty = match s.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).find(|t| *t != "null"),
        _ => None,
    };
    let ty = ty.or(if s.get("properties").is_some() {
        Some("object")
    } else {
        None
    });
    match ty {
        Some("object") => example_object(doc, s, depth),
        Some("array") => {
            let item = s
                .get("items")
                .map(|i| example_from_schema(doc, i, depth + 1))
                .unwrap_or(Value::Null);
            json!([item])
        }
        Some("string") => match s.get("format").and_then(Value::as_str) {
            Some("email") => json!("user@example.com"),
            Some("date-time") => json!("2024-01-01T00:00:00Z"),
            Some("date") => json!("2024-01-01"),
            Some("uuid") => json!("00000000-0000-4000-8000-000000000000"),
            Some("uri") | Some("url") => json!("https://example.com"),
            _ => json!("string"),
        },
        Some("integer") => s
            .get("minimum")
            .cloned()
            .filter(Value::is_i64)
            .unwrap_or(json!(0)),
        Some("number") => s
            .get("minimum")
            .cloned()
            .filter(Value::is_number)
            .unwrap_or(json!(0)),
        Some("boolean") => json!(true),
        Some("null") => Value::Null,
        _ => Value::Null,
    }
}

fn example_object(doc: &Value, s: &Value, depth: usize) -> Value {
    let mut m = Map::new();
    if let Some(props) = s.get("properties").and_then(Value::as_object) {
        for (k, v) in props {
            m.insert(k.clone(), example_from_schema(doc, v, depth + 1));
        }
    }
    Value::Object(m)
}

/// Example from a media type object: `example`, `examples`, or the schema.
fn media_example(doc: &Value, media: &Value) -> Option<Value> {
    let media = resolve(doc, media);
    if let Some(e) = media.get("example") {
        return Some(e.clone());
    }
    if let Some(ex) = media.get("examples") {
        let first = match ex {
            Value::Object(o) => o
                .values()
                .next()
                .map(|e| resolve(doc, e))
                .and_then(|e| e.get("value").cloned()),
            Value::Array(a) => a.first().cloned(),
            _ => None,
        };
        if first.is_some() {
            return first;
        }
    }
    media.get("schema").map(|s| example_from_schema(doc, s, 0))
}

fn pick_media(content: &Value) -> Option<&Value> {
    let o = content.as_object()?;
    o.iter()
        .find(|(k, _)| k.contains("json"))
        .or_else(|| o.iter().next())
        .map(|(_, v)| v)
}

fn operation_id_to_id(op_id: &str) -> String {
    let ok = !op_id.is_empty()
        && op_id
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            .unwrap_or(false)
        && op_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if ok {
        return op_id.to_string();
    }
    let mut out = String::new();
    let chars: Vec<char> = op_id.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_alphanumeric() {
            if c.is_uppercase()
                && i > 0
                && chars[i - 1].is_alphanumeric()
                && !chars[i - 1].is_uppercase()
                && !out.ends_with('.')
            {
                out.push('.');
            }
            out.extend(c.to_lowercase());
        } else if !out.ends_with('.') && !out.is_empty() {
            out.push('.');
        }
    }
    out.trim_matches('.').to_string()
}

const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch"];

/// Convert an OpenAPI document into endpoint definitions.
pub fn convert(doc: &Value, report: &mut ImportReport) -> Result<Vec<EndpointDef>> {
    let paths = match doc.get("paths").and_then(Value::as_object) {
        Some(p) => p,
        None => bail!("not an OpenAPI document: missing `paths`"),
    };
    let version = doc.get("openapi").and_then(Value::as_str).unwrap_or("");
    if !version.starts_with("3.") {
        report.warnings.push(format!(
            "document declares openapi `{}`; only 3.0/3.1 is fully supported",
            if version.is_empty() { "?" } else { version }
        ));
    }
    let mut defs = Vec::new();
    for (path, item) in paths {
        let item = resolve(doc, item);
        let shared: Vec<Value> = item
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for m in METHODS {
            let op = match item.get(*m) {
                Some(o) => o,
                None => continue,
            };
            let method = m.to_ascii_uppercase();
            let norm = naming::normalize_path(path);
            let id = op
                .get("x-ajiro-id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    op.get("operationId")
                        .and_then(Value::as_str)
                        .map(operation_id_to_id)
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| naming::endpoint_id(&method, &norm));

            let mut params_all = shared.clone();
            if let Some(a) = op.get("parameters").and_then(Value::as_array) {
                for p in a {
                    let rp = resolve(doc, p);
                    // operation level overrides path level with same name+in
                    let key = (rp.get("name").cloned(), rp.get("in").cloned());
                    params_all.retain(|q| {
                        let rq = resolve(doc, q);
                        (rq.get("name").cloned(), rq.get("in").cloned()) != key
                    });
                    params_all.push(p.clone());
                }
            }
            let mut req = RequestSpec {
                method: method.clone(),
                path: norm.clone(),
                ..Default::default()
            };
            for p in &params_all {
                let p = resolve(doc, p);
                let name = match p.get("name").and_then(Value::as_str) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let schema_ex = || p.get("schema").map(|s| example_from_schema(doc, s, 0));
                let val = p
                    .get("example")
                    .cloned()
                    .or_else(|| {
                        p.get("schema")
                            .and_then(|s| resolve(doc, s).get("default").cloned())
                    })
                    .or_else(|| {
                        p.get("schema")
                            .and_then(|s| resolve(doc, s).get("example").cloned())
                    });
                match p.get("in").and_then(Value::as_str) {
                    Some("path") => {
                        if let Some(v) = val.or_else(schema_ex) {
                            if !v.is_null() {
                                req.params.insert(name, v);
                            }
                        }
                    }
                    Some("query") => {
                        req.query.insert(name, val.unwrap_or(Value::Null));
                    }
                    Some("header")
                        if p.get("required").and_then(Value::as_bool).unwrap_or(false) =>
                    {
                        let v = val.or_else(schema_ex).unwrap_or(Value::Null);
                        let s = match v {
                            Value::String(s) => s,
                            Value::Null => String::new(),
                            o => o.to_string(),
                        };
                        req.headers.insert(name, s);
                    }
                    _ => {}
                }
            }
            if let Some(rb) = op.get("requestBody") {
                let rb = resolve(doc, rb);
                if let Some(media) = rb.get("content").and_then(pick_media) {
                    if let Some(ex) = media_example(doc, media) {
                        if !ex.is_null() {
                            req.body = Some(ex);
                            let ct = rb.get("content").and_then(Value::as_object).and_then(|o| {
                                o.iter()
                                    .find(|(_, v)| std::ptr::eq(*v, media))
                                    .map(|(k, _)| k.clone())
                            });
                            if let Some(ct) = ct {
                                if ct.contains("json") {
                                    req.headers.entry("Content-Type".into()).or_insert(ct);
                                }
                            }
                        }
                    }
                }
            }

            // responses
            let mut mock = MockSpec::default();
            let mut first_2xx: Option<u16> = None;
            let mut first_code: Option<u16> = None;
            if let Some(resps) = op.get("responses").and_then(Value::as_object) {
                for (code, r) in resps {
                    let status: u16 = match code.parse() {
                        Ok(c) => c,
                        Err(_) => {
                            if code != "default" {
                                report.warnings.push(format!(
                                    "{} {}: response `{}` skipped",
                                    method, path, code
                                ));
                            }
                            continue;
                        }
                    };
                    first_code.get_or_insert(status);
                    let r = resolve(doc, r);
                    let body = r
                        .get("content")
                        .and_then(pick_media)
                        .and_then(|m| media_example(doc, m))
                        .filter(|v| !v.is_null());
                    let resp = MockResponse {
                        status,
                        headers: IndexMap::new(),
                        body,
                        delay_ms: None,
                    };
                    if (200..300).contains(&status) && mock.default.is_none() {
                        first_2xx = Some(status);
                        mock.default = Some(resp);
                    } else {
                        mock.scenarios.insert(code.clone(), resp);
                    }
                }
            }
            let test_code = first_2xx.or(first_code).unwrap_or(200);
            let tags: Vec<String> = op
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            defs.push(EndpointDef {
                version: SCHEMA_VERSION,
                id,
                summary: op
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                description: op
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tags,
                task: None,
                depends_on: vec![],
                request: req,
                source: None,
                tests: vec![TestCase {
                    name: format!("returns {}", test_code),
                    expect: crate::model::Expect {
                        status: Some(StatusExpect::Code(test_code)),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                mock: if mock.default.is_some() || !mock.scenarios.is_empty() {
                    Some(mock)
                } else {
                    None
                },
            });
        }
    }
    Ok(defs)
}

/// Parse JSON or YAML text into a JSON value.
pub fn parse_document(text: &str) -> Result<Value> {
    let t = text.trim_start();
    if t.starts_with('{') {
        serde_json::from_str(text).context("invalid OpenAPI JSON")
    } else {
        serde_yaml::from_str(text).context("invalid OpenAPI YAML")
    }
}

pub fn import_str(project: &Project, text: &str, overwrite: bool) -> Result<ImportReport> {
    let doc = parse_document(text)?;
    let mut report = ImportReport::default();
    let defs = convert(&doc, &mut report)?;
    write_endpoints(project, defs, overwrite, &mut report)?;
    Ok(report)
}
