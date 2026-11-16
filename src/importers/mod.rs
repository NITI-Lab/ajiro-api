//! Postman / Bruno importers. (OWNER: integrations agent)
//!
//! Shared helpers live here (template conversion, URL cleanup); the actual
//! file writing is in [`write`] and is shared with the OpenAPI importer.

pub mod bruno;
pub mod postman;
pub mod write;

use crate::cli::ImportCommand;
use crate::config::Project;
use anyhow::Result;
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;

pub use write::{print_report, write_endpoints, ImportReport};

pub fn run_cli(dir: &Path, cmd: &ImportCommand) -> Result<i32> {
    let project = Project::discover_or_default(dir)?;
    match cmd {
        ImportCommand::Postman {
            file,
            overwrite,
            json,
        } => {
            let text = std::fs::read_to_string(file)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {}", file.display(), e))?;
            let report = postman::import_str(&project, &text, *overwrite)?;
            print_report(&report, *json)?;
        }
        ImportCommand::Bruno {
            dir: bdir,
            overwrite,
            json,
        } => {
            let report = bruno::import_dir(&project, bdir, *overwrite)?;
            print_report(&report, *json)?;
        }
    }
    Ok(crate::exit::OK)
}

fn template_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{\s*([^{}]*?)\s*\}\}").unwrap())
}

/// Convert `{{var}}` (Postman / Bruno) into Ajiro's `{{ var }}`. Dynamic
/// variables and `process.env.X` are mapped to their Ajiro equivalents.
pub fn convert_template(s: &str) -> String {
    template_re()
        .replace_all(s, |c: &regex::Captures| {
            let v = c[1].trim();
            let v = match v {
                "$guid" | "$randomUUID" => "$uuid".to_string(),
                "$isoTimestamp" => "$isoDate".to_string(),
                other => match other.strip_prefix("process.env.") {
                    Some(n) => format!("env.{}", n),
                    None => other.to_string(),
                },
            };
            format!("{{{{ {} }}}}", v)
        })
        .into_owned()
}

/// Apply [`convert_template`] to every string inside a JSON value.
pub fn convert_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(convert_template(s)),
        Value::Array(a) => Value::Array(a.iter().map(convert_value).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), convert_value(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Remove a leading `{{var}}` host or `scheme://host[:port]` from a URL and
/// drop the query string / fragment. The result always starts with `/`.
pub fn strip_host(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw.split('#').next().unwrap_or(raw);
    let raw = raw.split('?').next().unwrap_or(raw);
    static HOST: OnceLock<Regex> = OnceLock::new();
    let host = HOST.get_or_init(|| {
        Regex::new(r"^(?:[A-Za-z][A-Za-z0-9+.-]*://[^/]*|\{\{[^{}]*\}\}(?::\d+)?)").unwrap()
    });
    let rest = match host.find(raw) {
        Some(m) => &raw[m.end()..],
        None => {
            // `localhost:3000/users` / `api.example.com/users`
            let first = raw.split('/').next().unwrap_or("");
            if !raw.starts_with('/')
                && !first.starts_with(':')
                && (first.contains('.') || first.contains(':'))
            {
                &raw[first.len()..]
            } else {
                raw
            }
        }
    };
    if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{}", rest)
    }
}

/// Query string of a raw URL, as key/value pairs.
pub fn raw_query(raw: &str) -> Vec<(String, String)> {
    let raw = raw.split('#').next().unwrap_or(raw);
    match raw.split_once('?') {
        None => vec![],
        Some((_, q)) => q
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| match p.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (p.to_string(), String::new()),
            })
            .collect(),
    }
}

/// Convert `:id` and `{{id}}` path segments into `{id}` placeholders.
/// Returns the new path and the names of the placeholders (in order).
pub fn path_with_placeholders(path: &str) -> (String, Vec<String>) {
    let mut names = Vec::new();
    let segs: Vec<String> = path
        .split('/')
        .map(|s| {
            if let Some(n) = s.strip_prefix(':') {
                if !n.is_empty() {
                    names.push(n.to_string());
                    return format!("{{{}}}", n);
                }
            }
            let t = s.trim();
            if t.starts_with("{{") && t.ends_with("}}") && t.matches("{{").count() == 1 {
                let n = t[2..t.len() - 2].trim().trim_start_matches('$');
                let n: String = n
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                if !n.is_empty() {
                    names.push(n.clone());
                    return format!("{{{}}}", n);
                }
            }
            s.to_string()
        })
        .collect();
    (crate::naming::normalize_path(&segs.join("/")), names)
}

/// Parse a raw request body as JSON; on failure return the (template
/// converted) text as a string. The flag tells whether it was JSON.
pub fn parse_body_text(raw: &str) -> (Value, bool) {
    match serde_json::from_str::<Value>(raw) {
        Ok(v) => (convert_value(&v), true),
        Err(_) => (Value::String(convert_template(raw)), false),
    }
}

/// Default starter test for imports without information about the status.
pub fn starter_test_any_2xx() -> crate::model::TestCase {
    crate::model::TestCase {
        name: "responds with 2xx".into(),
        expect: crate::model::Expect {
            status: Some(crate::model::StatusExpect::AnyOf(vec![200, 201, 202, 204])),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn starter_test(code: u16) -> crate::model::TestCase {
    crate::model::TestCase {
        name: format!("returns {}", code),
        expect: crate::model::Expect {
            status: Some(crate::model::StatusExpect::Code(code)),
            ..Default::default()
        },
        ..Default::default()
    }
}
