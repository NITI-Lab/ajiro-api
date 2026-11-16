//! Postman Collection v2.1 importer.

use super::{
    convert_template, convert_value, parse_body_text, path_with_placeholders, raw_query,
    starter_test, starter_test_any_2xx, strip_host, write_endpoints, ImportReport,
};
use crate::config::Project;
use crate::model::{EndpointDef, MockResponse, MockSpec, RequestSpec, SCHEMA_VERSION};
use crate::naming;
use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use serde_json::Value;

pub fn import_str(project: &Project, text: &str, overwrite: bool) -> Result<ImportReport> {
    let coll: Value = serde_json::from_str(text).context("invalid Postman collection JSON")?;
    let (defs, mut report) = convert(&coll)?;
    write_endpoints(project, defs, overwrite, &mut report)?;
    Ok(report)
}

/// Convert a parsed collection into endpoint definitions (no I/O).
pub fn convert(coll: &Value) -> Result<(Vec<EndpointDef>, ImportReport)> {
    let items = match coll.get("item").and_then(Value::as_array) {
        Some(i) => i,
        None => bail!("not a Postman collection: missing `item` array"),
    };
    let mut report = ImportReport::default();
    if let Some(vars) = coll.get("variable").and_then(Value::as_array) {
        let mut m = IndexMap::new();
        for v in vars {
            if let Some(k) = v.get("key").and_then(Value::as_str) {
                let val = v.get("value").cloned().unwrap_or(Value::Null);
                m.insert(k.to_string(), convert_value(&val));
            }
        }
        if !m.is_empty() {
            report.suggested_variables.insert("local".into(), m);
        }
    }
    let auth = coll.get("auth").cloned();
    let mut defs = Vec::new();
    walk(items, &[], auth.as_ref(), &mut defs, &mut report);
    Ok((defs, report))
}

fn walk(
    items: &[Value],
    tags: &[String],
    auth: Option<&Value>,
    out: &mut Vec<EndpointDef>,
    report: &mut ImportReport,
) {
    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let item_auth = item.get("auth").or(auth);
        if let Some(children) = item.get("item").and_then(Value::as_array) {
            let mut t = tags.to_vec();
            if !name.is_empty() {
                t.push(name);
            }
            if has_scripts(item) {
                report.warnings.push(format!(
                    "folder `{}`: scripts are not converted",
                    t.join(" / ")
                ));
            }
            walk(children, &t, item_auth, out, report);
        } else if let Some(req) = item.get("request") {
            if let Some(def) = convert_request(item, req, &name, tags, item_auth, report) {
                out.push(def);
            }
        }
    }
}

fn has_scripts(item: &Value) -> bool {
    item.get("event")
        .and_then(Value::as_array)
        .map(|evs| {
            evs.iter().any(|e| {
                e.get("script")
                    .and_then(|s| s.get("exec"))
                    .map(|x| match x {
                        Value::Array(a) => a
                            .iter()
                            .any(|l| l.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)),
                        Value::String(s) => !s.trim().is_empty(),
                        _ => false,
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn kv_list(v: Option<&Value>) -> Vec<(String, String, bool)> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    let k = e.get("key").and_then(Value::as_str)?.to_string();
                    let val = match e.get("value") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Null) | None => String::new(),
                        Some(o) => o.to_string(),
                    };
                    let disabled = e.get("disabled").and_then(Value::as_bool).unwrap_or(false);
                    Some((k, val, disabled))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn bearer_token(auth: &Value) -> Option<String> {
    let arr = auth.get("bearer")?;
    let toks = match arr {
        Value::Array(a) => a
            .iter()
            .find(|e| e.get("key").and_then(Value::as_str) == Some("token"))
            .and_then(|e| e.get("value"))
            .and_then(Value::as_str)
            .map(str::to_string),
        Value::Object(o) => o.get("token").and_then(Value::as_str).map(str::to_string),
        _ => None,
    };
    Some(
        toks.filter(|t| !t.trim().is_empty())
            .map(|t| convert_template(&t))
            .unwrap_or_else(|| "{{ token }}".into()),
    )
}

fn convert_request(
    item: &Value,
    req: &Value,
    name: &str,
    tags: &[String],
    auth: Option<&Value>,
    report: &mut ImportReport,
) -> Option<EndpointDef> {
    // A request given as a plain URL string means GET.
    if let Some(url) = req.as_str() {
        return Some(build(
            name,
            tags,
            "GET",
            url,
            IndexMap::new(),
            None,
            None,
            None,
        ));
    }
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_ascii_uppercase();
    let url = req.get("url");
    let (raw_path, query_pairs, url_vars) = match url {
        Some(Value::String(s)) => (strip_host(s), raw_query(s), vec![]),
        Some(u @ Value::Object(_)) => {
            let raw = u.get("raw").and_then(Value::as_str).unwrap_or("");
            let path = match u.get("path") {
                Some(Value::Array(segs)) => {
                    let parts: Vec<String> = segs
                        .iter()
                        .map(|s| match s {
                            Value::String(s) => s.clone(),
                            o => o
                                .get("value")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        })
                        .collect();
                    format!("/{}", parts.join("/"))
                }
                Some(Value::String(s)) => format!("/{}", s.trim_start_matches('/')),
                _ => strip_host(raw),
            };
            let query = match u.get("query") {
                Some(_) => kv_list(u.get("query"))
                    .into_iter()
                    .filter(|(_, _, d)| !d)
                    .map(|(k, v, _)| (k, v))
                    .collect(),
                None => raw_query(raw),
            };
            let vars = kv_list(u.get("variable"))
                .into_iter()
                .map(|(k, v, _)| (k, v))
                .collect();
            (path, query, vars)
        }
        _ => {
            report
                .warnings
                .push(format!("request `{}` has no url; skipped", name));
            return None;
        }
    };
    let (path, placeholders) = path_with_placeholders(&raw_path);
    let mut params = IndexMap::new();
    for p in placeholders {
        let from_var = url_vars
            .iter()
            .find(|(k, _)| *k == p)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty());
        params.insert(
            p.clone(),
            Value::String(convert_template(
                &from_var.unwrap_or_else(|| format!("{{{{ {} }}}}", p)),
            )),
        );
    }

    let mut headers: IndexMap<String, String> = IndexMap::new();
    for (k, v, disabled) in kv_list(req.get("header")) {
        if !disabled {
            headers.insert(k, convert_template(&v));
        }
    }
    // auth (request level overrides inherited)
    let req_auth = req.get("auth").or(auth);
    if let Some(a) = req_auth {
        match a.get("type").and_then(Value::as_str) {
            Some("bearer") => {
                if let Some(t) = bearer_token(a) {
                    if !headers
                        .keys()
                        .any(|k| k.eq_ignore_ascii_case("authorization"))
                    {
                        headers.insert("Authorization".into(), format!("Bearer {}", t));
                    }
                }
            }
            Some("noauth") | None => {}
            Some(other) => report.warnings.push(format!(
                "request `{}`: auth type `{}` is not converted",
                name, other
            )),
        }
    }

    // body
    let mut body = None;
    if let Some(b) = req.get("body") {
        match b.get("mode").and_then(Value::as_str) {
            Some("raw") => {
                let raw = b.get("raw").and_then(Value::as_str).unwrap_or("");
                if !raw.trim().is_empty() {
                    let (v, is_json) = parse_body_text(raw);
                    let lang = b
                        .pointer("/options/raw/language")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if is_json {
                        if !headers
                            .keys()
                            .any(|k| k.eq_ignore_ascii_case("content-type"))
                        {
                            headers.insert("Content-Type".into(), "application/json".into());
                        }
                    } else if lang == "json" || raw.trim_start().starts_with('{') {
                        report.warnings.push(format!(
                            "request `{}`: body is not valid JSON (template variables as bare values?); kept as text",
                            name
                        ));
                    }
                    body = Some(v);
                }
            }
            Some(mode @ ("urlencoded" | "formdata")) => {
                let key = if mode == "urlencoded" {
                    "urlencoded"
                } else {
                    "formdata"
                };
                let mut obj = serde_json::Map::new();
                for e in b
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    if e.get("disabled").and_then(Value::as_bool).unwrap_or(false) {
                        continue;
                    }
                    let k = match e.get("key").and_then(Value::as_str) {
                        Some(k) => k.to_string(),
                        None => continue,
                    };
                    if e.get("type").and_then(Value::as_str) == Some("file") {
                        report.warnings.push(format!(
                            "request `{}`: file field `{}` is not converted",
                            name, k
                        ));
                        continue;
                    }
                    let v = e.get("value").and_then(Value::as_str).unwrap_or("");
                    obj.insert(k, Value::String(convert_template(v)));
                }
                report.warnings.push(format!(
                    "request `{}`: {} body converted to a JSON object",
                    name, mode
                ));
                body = Some(Value::Object(obj));
            }
            Some("graphql") => {
                report
                    .warnings
                    .push(format!("request `{}`: GraphQL body is not converted", name));
            }
            Some("file") | Some("binary") => {
                report
                    .warnings
                    .push(format!("request `{}`: file body is not converted", name));
            }
            _ => {}
        }
    }

    if has_scripts(item) {
        report.warnings.push(format!(
            "request `{}`: pre-request/test scripts are not converted",
            name
        ));
    }

    // saved responses -> mock
    let mut default: Option<MockResponse> = None;
    let mut scenarios: IndexMap<String, MockResponse> = IndexMap::new();
    let mut first_2xx: Option<u16> = None;
    for r in item
        .get("response")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let code = r.get("code").and_then(Value::as_u64).unwrap_or(200) as u16;
        let rname = r.get("name").and_then(Value::as_str).unwrap_or("");
        let (body, is_json) = match r.get("body").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => {
                let (v, j) = parse_body_text(s);
                (Some(v), j)
            }
            _ => (None, true),
        };
        let mut rh = IndexMap::new();
        if !is_json {
            if let Some((_, v, _)) = kv_list(r.get("header"))
                .into_iter()
                .find(|(k, _, _)| k.eq_ignore_ascii_case("content-type"))
            {
                rh.insert("Content-Type".to_string(), v);
            }
        }
        let resp = MockResponse {
            status: code,
            headers: rh,
            body,
            delay_ms: None,
        };
        if (200..300).contains(&code) && default.is_none() {
            default = Some(resp);
            first_2xx = Some(code);
        } else {
            let mut key = code.to_string();
            if scenarios.contains_key(&key) {
                let s = naming::slug(rname);
                key = format!("{}-{}", code, s);
                let mut n = 2;
                while scenarios.contains_key(&key) {
                    key = format!("{}-{}-{}", code, s, n);
                    n += 1;
                }
            }
            scenarios.insert(key, resp);
        }
    }
    let mock = if default.is_some() || !scenarios.is_empty() {
        Some(MockSpec { default, scenarios })
    } else {
        None
    };
    let test = match first_2xx {
        Some(c) => starter_test(c),
        None => starter_test_any_2xx(),
    };

    let description = match item.get("request").and_then(|r| r.get("description")) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(o) => o
            .get("content")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        None => None,
    };

    let mut def = build(name, tags, &method, &path, headers, body, mock, Some(test));
    def.request.params = params;
    for (k, v) in query_pairs {
        def.request
            .query
            .insert(k, Value::String(convert_template(&v)));
    }
    def.description = description;
    Some(def)
}

#[allow(clippy::too_many_arguments)]
fn build(
    name: &str,
    tags: &[String],
    method: &str,
    path: &str,
    headers: IndexMap<String, String>,
    body: Option<Value>,
    mock: Option<MockSpec>,
    test: Option<crate::model::TestCase>,
) -> EndpointDef {
    let path = if path.starts_with('/') {
        naming::normalize_path(path)
    } else {
        naming::normalize_path(&strip_host(path))
    };
    EndpointDef {
        version: SCHEMA_VERSION,
        id: naming::endpoint_id(method, &path),
        summary: if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        },
        description: None,
        tags: tags.to_vec(),
        task: None,
        depends_on: vec![],
        request: RequestSpec {
            method: method.to_string(),
            path,
            params: IndexMap::new(),
            query: IndexMap::new(),
            headers,
            body,
        },
        source: None,
        tests: test.into_iter().collect(),
        mock,
    }
}
