//! `{{ name }}` templating: variables, captured values, `env.NAME` secrets and
//! `$builtin` generators.

use indexmap::IndexMap;
use serde_json::Value;
use std::collections::HashMap;

/// Template evaluation context.
#[derive(Debug, Default, Clone)]
pub struct Vars {
    /// Values loaded from the dotenv file (only reachable as `env.NAME`).
    pub dotenv: HashMap<String, String>,
    /// Environment variables and captured values.
    pub vars: IndexMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TemplateError {
    Undefined(String),
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::Undefined(name) => {
                if let Some(n) = name.strip_prefix("env.") {
                    write!(
                        f,
                        "undefined variable `{{{{ {} }}}}`: `{}` is not set in the .env file or in the process environment",
                        name, n
                    )
                } else {
                    write!(
                        f,
                        "undefined variable `{{{{ {} }}}}`: define it under environments.<env>.variables in ajiro-api.yaml, \
                         capture it in an earlier test (capture: {}: $.path) and use depends_on, or reference a secret as {{{{ env.NAME }}}} from .env",
                        name, name
                    )
                }
            }
        }
    }
}

impl std::error::Error for TemplateError {}

impl Vars {
    fn lookup(&self, name: &str) -> Option<Value> {
        if let Some(n) = name.strip_prefix("env.") {
            return self
                .dotenv
                .get(n)
                .cloned()
                .or_else(|| std::env::var(n).ok())
                .map(Value::String);
        }
        if let Some(b) = builtin(name) {
            return Some(b);
        }
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        if name.contains('.') {
            let parts: Vec<&str> = name.split('.').collect();
            for i in (1..parts.len()).rev() {
                let prefix = parts[..i].join(".");
                if let Some(root) = self.vars.get(&prefix) {
                    let mut cur = root;
                    let mut ok = true;
                    for p in &parts[i..] {
                        let next = match cur {
                            Value::Object(m) => m.get(*p),
                            Value::Array(a) => p.parse::<usize>().ok().and_then(|n| a.get(n)),
                            _ => None,
                        };
                        match next {
                            Some(n) => cur = n,
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        return Some(cur.clone());
                    }
                }
            }
        }
        None
    }

    /// Does the variable exist (used for path parameter fallback)?
    pub fn get(&self, name: &str) -> Option<Value> {
        self.lookup(name)
    }
}

fn rand_u64() -> u64 {
    let b = uuid::Uuid::new_v4();
    let bytes = b.as_bytes();
    u64::from_le_bytes(bytes[..8].try_into().unwrap())
}

fn builtin(name: &str) -> Option<Value> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
    };
    match name {
        "$uuid" => Some(Value::String(uuid::Uuid::new_v4().to_string())),
        "$timestamp" => Some(Value::from(now().as_secs())),
        "$timestampMs" => Some(Value::from(now().as_millis() as u64)),
        "$isoDate" => Some(Value::String(iso_date(now().as_secs()))),
        "$randomInt" => Some(Value::from(rand_u64() % 1_000_000)),
        "$randomEmail" => Some(Value::String(format!(
            "user{}@example.com",
            rand_u64() % 100_000_000
        ))),
        _ => None,
    }
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ` from unix seconds (civil-from-days algorithm).
fn iso_date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Text form of a value when interpolated into a larger string.
pub fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Render one string. If the whole string is exactly one placeholder the
/// value keeps its JSON type; otherwise the result is a string.
pub fn render_str(s: &str, vars: &Vars) -> Result<Value, TemplateError> {
    // Fast path
    if !s.contains("{{") {
        return Ok(Value::String(s.to_string()));
    }
    let mut parts: Vec<Result<&str, String>> = Vec::new(); // Ok(literal) | Err(name)
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        match rest[start + 2..].find("}}") {
            Some(len) => {
                if start > 0 {
                    parts.push(Ok(&rest[..start]));
                }
                let name = rest[start + 2..start + 2 + len].trim().to_string();
                parts.push(Err(name));
                rest = &rest[start + 2 + len + 2..];
            }
            None => break,
        }
    }
    if !rest.is_empty() {
        parts.push(Ok(rest));
    }
    if parts.len() == 1 {
        if let Err(name) = &parts[0] {
            return vars
                .lookup(name)
                .ok_or_else(|| TemplateError::Undefined(name.clone()));
        }
    }
    let mut out = String::new();
    for p in parts {
        match p {
            Ok(lit) => out.push_str(lit),
            Err(name) => {
                let v = vars.lookup(&name).ok_or(TemplateError::Undefined(name))?;
                out.push_str(&value_to_text(&v));
            }
        }
    }
    Ok(Value::String(out))
}

/// Render a string and return its text form.
pub fn render_text(s: &str, vars: &Vars) -> Result<String, TemplateError> {
    render_str(s, vars).map(|v| value_to_text(&v))
}

/// Render every string inside a JSON value (object keys are left alone).
pub fn render_value(v: &Value, vars: &Vars) -> Result<Value, TemplateError> {
    Ok(match v {
        Value::String(s) => render_str(s, vars)?,
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|x| render_value(x, vars))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, x) in m {
                out.insert(k.clone(), render_value(x, vars)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vars() -> Vars {
        let mut v = Vars::default();
        v.dotenv.insert("PASSWORD".into(), "hunter22".into());
        v.vars.insert("userId".into(), json!(42));
        v.vars.insert("name".into(), json!("alice"));
        v.vars.insert("auth.token".into(), json!("t0k"));
        v.vars.insert(
            "user".into(),
            json!({"profile": {"email": "a@b.c"}, "tags": ["x", "y"]}),
        );
        v
    }

    #[test]
    fn interpolates_and_keeps_types() {
        let v = vars();
        assert_eq!(
            render_str("hi {{ name }}!", &v).unwrap(),
            json!("hi alice!")
        );
        assert_eq!(render_str("{{userId}}", &v).unwrap(), json!(42));
        assert_eq!(render_str("id={{ userId }}", &v).unwrap(), json!("id=42"));
        assert_eq!(
            render_str("{{ env.PASSWORD }}", &v).unwrap(),
            json!("hunter22")
        );
        assert_eq!(render_str("{{ auth.token }}", &v).unwrap(), json!("t0k"));
        assert_eq!(
            render_str("{{ user.profile.email }}", &v).unwrap(),
            json!("a@b.c")
        );
        assert_eq!(render_str("{{ user.tags.1 }}", &v).unwrap(), json!("y"));
        assert_eq!(
            render_str("no braces {{ unclosed", &v).unwrap(),
            json!("no braces {{ unclosed")
        );
    }

    #[test]
    fn undefined_variable_message() {
        let e = render_str("{{ missing }}", &vars()).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("missing"), "{}", msg);
        assert!(msg.contains("variables"), "{}", msg);
        let e = render_str("x {{ env.NOPE_NOT_SET_12345 }}", &vars()).unwrap_err();
        assert!(e.to_string().contains(".env"));
    }

    #[test]
    fn builtins() {
        let v = Vars::default();
        assert!(
            render_str("{{ $uuid }}", &v)
                .unwrap()
                .as_str()
                .unwrap()
                .len()
                == 36
        );
        assert!(render_str("{{ $timestamp }}", &v).unwrap().is_u64());
        assert!(render_str("{{ $timestampMs }}", &v).unwrap().is_u64());
        assert!(render_str("{{ $randomInt }}", &v).unwrap().is_u64());
        assert!(render_str("{{ $randomEmail }}", &v)
            .unwrap()
            .as_str()
            .unwrap()
            .contains('@'));
        let d = render_str("{{ $isoDate }}", &v).unwrap();
        assert!(regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")
            .unwrap()
            .is_match(d.as_str().unwrap()));
        assert_eq!(iso_date(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_date(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn renders_nested() {
        let out = render_value(
            &json!({"a": ["{{ userId }}", {"b": "x{{ name }}"}], "n": 1}),
            &vars(),
        )
        .unwrap();
        assert_eq!(out, json!({"a": [42, {"b": "xalice"}], "n": 1}));
    }
}
