//! Masking of secrets in everything that ends up in a report.

use serde_json::Value;
use std::collections::HashSet;

pub const MASK: &str = "***";

const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "proxy-authorization",
];

#[derive(Debug, Default, Clone)]
pub struct Redactor {
    /// Extra names from config `redact` (normalized).
    extra: HashSet<String>,
    /// Exact secret values to replace inside strings (len >= 4).
    secrets: Vec<String>,
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

impl Redactor {
    pub fn new(extra_names: &[String], secrets: impl IntoIterator<Item = String>) -> Self {
        let mut r = Redactor {
            extra: extra_names.iter().map(|n| normalize(n)).collect(),
            secrets: vec![],
        };
        for s in secrets {
            r.add_secret(&s);
        }
        r
    }

    /// Register a value (e.g. a captured token) that must never be printed.
    pub fn add_secret(&mut self, s: &str) {
        if s.len() >= 4 && !self.secrets.iter().any(|x| x == s) {
            self.secrets.push(s.to_string());
            // longest first so that overlapping secrets are fully replaced
            self.secrets.sort_by_key(|x| std::cmp::Reverse(x.len()));
        }
    }

    /// Is `key` (JSON key, capture name, header name) a secret by name?
    pub fn is_sensitive(&self, key: &str) -> bool {
        let n = normalize(key);
        if n.is_empty() {
            return false;
        }
        n.contains("password")
            || n.contains("passwd")
            || n.contains("secret")
            || n.ends_with("token")
            || n.ends_with("apikey")
            || self.extra.contains(&n)
    }

    fn is_sensitive_header(&self, name: &str) -> bool {
        SENSITIVE_HEADERS.contains(&name.to_ascii_lowercase().as_str()) || self.is_sensitive(name)
    }

    /// Replace known secret values inside a string.
    pub fn mask_str(&self, s: &str) -> String {
        let mut out = s.to_string();
        for sec in &self.secrets {
            if out.contains(sec.as_str()) {
                out = out.replace(sec.as_str(), MASK);
            }
        }
        out
    }

    pub fn mask_header(&self, name: &str, value: &str) -> String {
        if self.is_sensitive_header(name) {
            let lower = value.trim_start().to_ascii_lowercase();
            if name.eq_ignore_ascii_case("authorization")
                || name.eq_ignore_ascii_case("proxy-authorization")
            {
                if lower.starts_with("bearer ") {
                    return format!("Bearer {}", MASK);
                }
                if lower.starts_with("basic ") {
                    return format!("Basic {}", MASK);
                }
            }
            return MASK.to_string();
        }
        self.mask_str(value)
    }

    pub fn mask_value(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.mask_str(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.mask_value(x)).collect()),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, x)| {
                        if self.is_sensitive(k) && !x.is_null() {
                            (k.clone(), Value::String(MASK.into()))
                        } else {
                            (k.clone(), self.mask_value(x))
                        }
                    })
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn masks_by_name_and_value() {
        let r = Redactor::new(
            &["x-internal".to_string()],
            vec!["s3cr3t-value".to_string(), "ab".to_string()],
        );
        assert_eq!(
            r.mask_header("Authorization", "Bearer abc.def"),
            "Bearer ***"
        );
        assert_eq!(r.mask_header("X-Api-Key", "k"), "***");
        assert_eq!(r.mask_header("X-Internal", "k"), "***");
        assert_eq!(
            r.mask_header("X-Other", "has s3cr3t-value inside"),
            "has *** inside"
        );
        let v = r.mask_value(&json!({"user": {"password": "p", "accessToken": "t", "name": "n s3cr3t-value"}, "list": [{"api_key": "k"}]}));
        assert_eq!(
            v,
            json!({"user": {"password": "***", "accessToken": "***", "name": "n ***"}, "list": [{"api_key": "***"}]})
        );
        // short secrets are ignored
        assert_eq!(r.mask_str("about"), "about");
    }
}
