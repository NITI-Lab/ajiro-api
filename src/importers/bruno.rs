//! Bruno collection (`.bru` files) importer.

use super::{
    convert_template, convert_value, parse_body_text, path_with_placeholders, raw_query,
    starter_test_any_2xx, strip_host, write_endpoints, ImportReport,
};
use crate::config::Project;
use crate::model::RequestSpec;
use crate::model::{BodyExpect, EndpointDef, Expect, StatusExpect, TestCase, SCHEMA_VERSION};
use anyhow::{bail, Result};
use indexmap::IndexMap;
use serde_json::Value;
use std::path::Path;

/// One `name { ... }` block of a `.bru` file.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub name: String,
    /// Raw text between the braces (without the surrounding lines).
    pub text: String,
}

/// Split a `.bru` file into its top-level blocks.
///
/// Blocks start with `name {` at column 0 and end with `}` at column 0.
/// `body:*` blocks additionally track brace depth (string aware) so that
/// unindented JSON inside them does not terminate the block early.
pub fn parse_blocks(src: &str) -> Vec<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim_end();
        if let Some(name) = t.strip_suffix('{').map(str::trim) {
            if !line.starts_with(char::is_whitespace)
                && !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, ':' | '-' | '_'))
            {
                let track_depth = name.starts_with("body");
                let mut depth = 1i32;
                let mut in_str = false;
                let mut body = Vec::new();
                i += 1;
                while i < lines.len() {
                    let l = lines[i];
                    if track_depth {
                        let mut escaped = false;
                        for ch in l.chars() {
                            if in_str {
                                if escaped {
                                    escaped = false;
                                } else if ch == '\\' {
                                    escaped = true;
                                } else if ch == '"' {
                                    in_str = false;
                                }
                            } else if ch == '"' {
                                in_str = true;
                            } else if ch == '{' {
                                depth += 1;
                            } else if ch == '}' {
                                depth -= 1;
                            }
                        }
                        if depth <= 0 {
                            if l.trim() != "}" {
                                body.push(l);
                            }
                            break;
                        }
                    } else if l.trim_end() == "}" {
                        break;
                    }
                    body.push(l);
                    i += 1;
                }
                blocks.push(Block {
                    name: name.to_string(),
                    text: body.join("\n"),
                });
            }
        }
        i += 1;
    }
    blocks
}

/// Parse a dictionary block (`key: value` lines). Returns (key, value, enabled).
pub fn parse_dict(text: &str) -> Vec<(String, String, bool)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() {
                return None;
            }
            let (k, v) = l.split_once(':')?;
            let k = k.trim();
            let (k, enabled) = match k.strip_prefix('~') {
                Some(k) => (k.trim(), false),
                None => (k, true),
            };
            Some((k.to_string(), v.trim().to_string(), enabled))
        })
        .collect()
}

fn dedent(text: &str) -> String {
    let indent = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    text.lines()
        .map(|l| {
            if l.len() >= indent {
                &l[indent..]
            } else {
                l.trim_start()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

const METHODS: &[&str] = &["get", "post", "put", "patch", "delete", "options", "head"];

/// Convert one `.bru` file (given its tags from the directory structure).
pub fn convert_bru(
    src: &str,
    file_label: &str,
    tags: &[String],
    report: &mut ImportReport,
) -> Option<EndpointDef> {
    let blocks = parse_blocks(src);
    let find = |n: &str| blocks.iter().find(|b| b.name == n);
    let (mblock, method) = match blocks.iter().find(|b| METHODS.contains(&b.name.as_str())) {
        Some(b) => (b, b.name.to_ascii_uppercase()),
        None => {
            report.warnings.push(format!(
                "{}: no HTTP method block found; skipped",
                file_label
            ));
            return None;
        }
    };
    let mdict = parse_dict(&mblock.text);
    let get = |k: &str| {
        mdict
            .iter()
            .find(|(key, _, en)| key == k && *en)
            .map(|(_, v, _)| v.clone())
    };
    let url = get("url").unwrap_or_default();
    let body_mode = get("body").unwrap_or_else(|| "none".into());
    let auth_mode = get("auth").unwrap_or_else(|| "none".into());

    let name = find("meta")
        .and_then(|b| {
            parse_dict(&b.text)
                .into_iter()
                .find(|(k, _, _)| k == "name")
                .map(|(_, v, _)| v)
        })
        .filter(|s| !s.is_empty());

    let (path_raw, placeholders) = path_with_placeholders(&strip_host(&url));
    let mut params: IndexMap<String, Value> = IndexMap::new();
    let path_params: Vec<(String, String, bool)> = find("params:path")
        .map(|b| parse_dict(&b.text))
        .unwrap_or_default();
    for p in placeholders {
        let v = path_params
            .iter()
            .find(|(k, _, en)| *k == p && *en)
            .map(|(_, v, _)| convert_template(v))
            .unwrap_or_else(|| format!("{{{{ {} }}}}", p));
        params.insert(p, Value::String(v));
    }

    let mut query: IndexMap<String, Value> = IndexMap::new();
    match find("params:query") {
        Some(b) => {
            for (k, v, en) in parse_dict(&b.text) {
                if en {
                    query.insert(k, Value::String(convert_template(&v)));
                }
            }
        }
        None => {
            for (k, v) in raw_query(&url) {
                query.insert(k, Value::String(convert_template(&v)));
            }
        }
    }

    let mut headers: IndexMap<String, String> = IndexMap::new();
    if let Some(b) = find("headers") {
        for (k, v, en) in parse_dict(&b.text) {
            if en {
                headers.insert(k, convert_template(&v));
            }
        }
    }

    match auth_mode.as_str() {
        "bearer" => {
            let tok = find("auth:bearer")
                .and_then(|b| {
                    parse_dict(&b.text)
                        .into_iter()
                        .find(|(k, _, _)| k == "token")
                        .map(|(_, v, _)| v)
                })
                .filter(|t| !t.is_empty())
                .map(|t| convert_template(&t))
                .unwrap_or_else(|| "{{ token }}".into());
            headers
                .entry("Authorization".into())
                .or_insert(format!("Bearer {}", tok));
        }
        "none" | "inherit" => {}
        other => report.warnings.push(format!(
            "{}: auth mode `{}` is not converted",
            file_label, other
        )),
    }

    let mut body: Option<Value> = None;
    match body_mode.as_str() {
        "json" => {
            if let Some(b) = find("body:json") {
                let raw = dedent(&b.text);
                if !raw.is_empty() {
                    let (v, is_json) = parse_body_text(&raw);
                    if !is_json {
                        report.warnings.push(format!(
                            "{}: body is not valid JSON (template variables as bare values?); kept as text",
                            file_label
                        ));
                    } else if !headers
                        .keys()
                        .any(|k| k.eq_ignore_ascii_case("content-type"))
                    {
                        headers.insert("Content-Type".into(), "application/json".into());
                    }
                    body = Some(v);
                }
            }
        }
        "text" | "xml" | "sparql" => {
            if let Some(b) = find(&format!("body:{}", body_mode)) {
                body = Some(Value::String(convert_template(&dedent(&b.text))));
            }
        }
        "formUrlEncoded" | "multipartForm" => {
            let bn = if body_mode == "formUrlEncoded" {
                "body:form-urlencoded"
            } else {
                "body:multipart-form"
            };
            if let Some(b) = find(bn) {
                let mut obj = serde_json::Map::new();
                for (k, v, en) in parse_dict(&b.text) {
                    if en {
                        obj.insert(k, Value::String(convert_template(&v)));
                    }
                }
                report.warnings.push(format!(
                    "{}: {} body converted to a JSON object",
                    file_label, bn
                ));
                body = Some(Value::Object(obj));
            }
        }
        "none" => {}
        other => report.warnings.push(format!(
            "{}: body mode `{}` is not converted",
            file_label, other
        )),
    }

    // vars:pre-request -> suggested variables
    if let Some(b) = find("vars:pre-request") {
        let entry = report
            .suggested_variables
            .entry("local".into())
            .or_default();
        for (k, v, en) in parse_dict(&b.text) {
            if en {
                entry.entry(k).or_insert(convert_value(&Value::String(v)));
            }
        }
    }

    // assertions -> expect
    let mut expect = Expect::default();
    if let Some(b) = find("assert") {
        for (k, v, en) in parse_dict(&b.text) {
            if !en {
                continue;
            }
            let (op, rhs) = match v.split_once(char::is_whitespace) {
                Some((o, r)) => (o.trim(), r.trim()),
                None => (v.as_str(), ""),
            };
            if k == "res.status" && op == "eq" {
                if let Ok(c) = rhs.parse::<u16>() {
                    expect.status = Some(StatusExpect::Code(c));
                    continue;
                }
            } else if let Some(p) = k.strip_prefix("res.body.") {
                if op == "eq" {
                    let val = serde_json::from_str::<Value>(rhs)
                        .unwrap_or_else(|_| Value::String(rhs.to_string()));
                    let path = format!("$.{}", p);
                    expect
                        .body
                        .get_or_insert_with(BodyExpect::default)
                        .match_
                        .insert(path, convert_value(&val));
                    continue;
                }
            }
            report.warnings.push(format!(
                "{}: assertion `{}: {}` is not converted",
                file_label, k, v
            ));
        }
    }
    for b in &blocks {
        if b.name.starts_with("script:") || b.name == "tests" {
            report.warnings.push(format!(
                "{}: `{}` block is not converted",
                file_label, b.name
            ));
        }
    }

    let test = if expect.status.is_some() || expect.body.is_some() {
        if expect.status.is_none() {
            expect.status = Some(StatusExpect::AnyOf(vec![200, 201, 202, 204]));
        }
        let label = match &expect.status {
            Some(StatusExpect::Code(c)) => format!("returns {}", c),
            _ => "responds with 2xx".to_string(),
        };
        TestCase {
            name: label,
            expect,
            ..Default::default()
        }
    } else {
        starter_test_any_2xx()
    };

    let description = find("docs")
        .map(|b| dedent(&b.text))
        .filter(|s| !s.is_empty());
    let id = crate::naming::endpoint_id(&method, &path_raw);
    Some(EndpointDef {
        version: SCHEMA_VERSION,
        id,
        summary: name,
        description,
        tags: tags.to_vec(),
        task: None,
        depends_on: vec![],
        request: RequestSpec {
            method,
            path: path_raw,
            params,
            query,
            headers,
            body,
        },
        source: None,
        tests: vec![test],
        mock: None,
    })
}

/// Import a Bruno collection directory.
pub fn import_dir(project: &Project, dir: &Path, overwrite: bool) -> Result<ImportReport> {
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let mut report = ImportReport::default();
    let mut defs = Vec::new();
    let mut files: Vec<_> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(e.file_type().is_dir() && (n == "node_modules" || n.starts_with('.')))
        })
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file() && e.path().extension().and_then(|x| x.to_str()) == Some("bru")
        })
        .map(|e| e.into_path())
        .collect();
    files.sort();
    for f in files {
        let rel = f.strip_prefix(dir).unwrap_or(&f).to_path_buf();
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        let fname = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let in_env = rel
            .components()
            .next()
            .map(|c| c.as_os_str() == "environments")
            .unwrap_or(false);
        if in_env {
            let env_name = f
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("env")
                .to_string();
            let src = std::fs::read_to_string(&f)?;
            let mut vars: IndexMap<String, Value> = IndexMap::new();
            for b in parse_blocks(&src) {
                if b.name == "vars" {
                    for (k, v, en) in parse_dict(&b.text) {
                        if en {
                            vars.insert(k, Value::String(convert_template(&v)));
                        }
                    }
                }
            }
            // `vars:secret [ ... ]` list form
            if let Some(start) = src.find("vars:secret") {
                if let Some(rest) = src[start..].split_once('[').map(|x| x.1) {
                    if let Some(list) = rest.split_once(']').map(|x| x.0) {
                        for n in list
                            .split(',')
                            .map(|s| s.trim().trim_start_matches('~'))
                            .filter(|s| !s.is_empty())
                        {
                            vars.entry(n.to_string()).or_insert(Value::String(format!(
                                "{{{{ env.{} }}}}",
                                n.to_uppercase()
                            )));
                        }
                    }
                }
            }
            if !vars.is_empty() {
                report.suggested_variables.insert(env_name, vars);
            }
            continue;
        }
        if fname == "collection.bru" || fname == "folder.bru" {
            continue;
        }
        let src = std::fs::read_to_string(&f)?;
        let tags: Vec<String> = rel
            .parent()
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(d) = convert_bru(&src, &rel_s, &tags, &mut report) {
            defs.push(d);
        }
    }
    write_endpoints(project, defs, overwrite, &mut report)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_json_body() {
        let src = "meta {\n  name: x\n}\n\npost {\n  url: {{baseUrl}}/users\n  body: json\n}\n\nbody:json {\n  {\n    \"a\": {\"b\": 1},\n    \"c\": [{\"d\": \"}\"}]\n  }\n}\n";
        let b = parse_blocks(src);
        assert_eq!(b.len(), 3);
        let body = b.iter().find(|b| b.name == "body:json").unwrap();
        let v: Value = serde_json::from_str(&dedent(&body.text)).unwrap();
        assert_eq!(v["a"]["b"], 1);
    }
}
