//! JSONPath helpers: querying (via `serde_json_path`), removal / replacement of
//! simple paths (used for `ignore`), path formatting and deep comparison.

use serde_json::Value;
use serde_json_path::JsonPath;

/// First match of `path` in `value`. `Err` = invalid JSONPath.
pub fn query_first<'a>(value: &'a Value, path: &str) -> Result<Option<&'a Value>, String> {
    let p = JsonPath::parse(path).map_err(|e| format!("invalid JSONPath `{}`: {}", path, e))?;
    Ok(p.query(value).first())
}

/// True when `path` can select more than one node (wildcards, recursive
/// descent, filters, slices, unions).
pub fn is_multi(path: &str) -> bool {
    path.contains('*')
        || path.contains("..")
        || path.contains("[?")
        || path.contains(':')
        || path.contains(',')
}

/// All matches of `path`, cloned. `Err` = invalid JSONPath.
pub fn query_all(value: &Value, path: &str) -> Result<Vec<Value>, String> {
    let p = JsonPath::parse(path).map_err(|e| format!("invalid JSONPath `{}`: {}", path, e))?;
    Ok(p.query(value).all().into_iter().cloned().collect())
}

// ---------------------------------------------------------------- mutate ---

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Key(String),
    Idx(usize),
    Wild,
    /// `..name`
    Desc(String),
}

#[derive(Debug, Clone)]
pub enum Op {
    Remove,
    Replace(Value),
}

/// Parse the subset of JSONPath supported for `ignore`:
/// `$.a.b`, `$['a']`, `$.a[0].b`, `$.items[*].id`, `$.a.*`, `$..id`.
fn parse_simple(path: &str) -> Option<Vec<Tok>> {
    let p = path.trim();
    let rest = p.strip_prefix('$')?;
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0;
    let mut toks = Vec::new();
    while i < chars.len() {
        match chars[i] {
            '.' => {
                let desc = chars.get(i + 1) == Some(&'.');
                i += if desc { 2 } else { 1 };
                let start = i;
                while i < chars.len() && chars[i] != '.' && chars[i] != '[' {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect();
                if desc {
                    if name.is_empty() || name == "*" {
                        return None;
                    }
                    toks.push(Tok::Desc(name));
                } else if name == "*" {
                    toks.push(Tok::Wild);
                } else if name.is_empty() {
                    return None;
                } else {
                    toks.push(Tok::Key(name));
                }
            }
            '[' => {
                let end = chars[i..].iter().position(|c| *c == ']')? + i;
                let inner: String = chars[i + 1..end].iter().collect();
                let inner = inner.trim();
                if inner == "*" {
                    toks.push(Tok::Wild);
                } else if let Ok(n) = inner.parse::<usize>() {
                    toks.push(Tok::Idx(n));
                } else if (inner.starts_with('\'') && inner.ends_with('\'') && inner.len() >= 2)
                    || (inner.starts_with('"') && inner.ends_with('"') && inner.len() >= 2)
                {
                    toks.push(Tok::Key(inner[1..inner.len() - 1].to_string()));
                } else {
                    return None;
                }
                i = end + 1;
            }
            _ => return None,
        }
    }
    Some(toks)
}

/// Remove or replace every node selected by `path`. Returns `false` when the
/// path uses syntax outside the supported subset (nothing is changed).
pub fn modify(value: &mut Value, path: &str, op: &Op) -> bool {
    match parse_simple(path) {
        Some(toks) if !toks.is_empty() => {
            apply(value, &toks, op);
            true
        }
        Some(_) => {
            // `$` itself
            match op {
                Op::Remove => *value = Value::Null,
                Op::Replace(v) => *value = v.clone(),
            }
            true
        }
        None => false,
    }
}

fn leaf(parent: &mut Value, tok: &Tok, op: &Op) {
    match (parent, tok) {
        (Value::Object(m), Tok::Key(k)) => match op {
            Op::Remove => {
                m.shift_remove(k);
            }
            Op::Replace(v) => {
                if let Some(slot) = m.get_mut(k) {
                    *slot = v.clone();
                }
            }
        },
        (Value::Array(a), Tok::Idx(n)) => match op {
            Op::Remove => {
                if *n < a.len() {
                    a.remove(*n);
                }
            }
            Op::Replace(v) => {
                if let Some(slot) = a.get_mut(*n) {
                    *slot = v.clone();
                }
            }
        },
        (Value::Object(m), Tok::Wild) => match op {
            Op::Remove => m.clear(),
            Op::Replace(v) => m.values_mut().for_each(|s| *s = v.clone()),
        },
        (Value::Array(a), Tok::Wild) => match op {
            Op::Remove => a.clear(),
            Op::Replace(v) => a.iter_mut().for_each(|s| *s = v.clone()),
        },
        _ => {}
    }
}

fn apply(v: &mut Value, toks: &[Tok], op: &Op) {
    let Some((first, rest)) = toks.split_first() else {
        return;
    };
    if let Tok::Desc(name) = first {
        desc(v, name, rest, op);
        return;
    }
    if rest.is_empty() {
        leaf(v, first, op);
        return;
    }
    match (v, first) {
        (Value::Object(m), Tok::Key(k)) => {
            if let Some(c) = m.get_mut(k) {
                apply(c, rest, op);
            }
        }
        (Value::Array(a), Tok::Idx(n)) => {
            if let Some(c) = a.get_mut(*n) {
                apply(c, rest, op);
            }
        }
        (Value::Object(m), Tok::Wild) => m.values_mut().for_each(|c| apply(c, rest, op)),
        (Value::Array(a), Tok::Wild) => a.iter_mut().for_each(|c| apply(c, rest, op)),
        _ => {}
    }
}

fn desc(v: &mut Value, name: &str, rest: &[Tok], op: &Op) {
    match v {
        Value::Object(m) => {
            if m.contains_key(name) {
                if rest.is_empty() {
                    match op {
                        Op::Remove => {
                            m.shift_remove(name);
                        }
                        Op::Replace(r) => {
                            if let Some(s) = m.get_mut(name) {
                                *s = r.clone();
                            }
                        }
                    }
                } else if let Some(c) = m.get_mut(name) {
                    apply(c, rest, op);
                }
            }
            for (k, c) in m.iter_mut() {
                if k != name || !rest.is_empty() {
                    desc(c, name, rest, op);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|c| desc(c, name, rest, op)),
        _ => {}
    }
}

// ------------------------------------------------------------ formatting ---

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

pub fn child(path: &str, key: &str) -> String {
    if is_ident(key) {
        format!("{}.{}", path, key)
    } else {
        format!("{}['{}']", path, key.replace('\'', "\\'"))
    }
}

pub fn index(path: &str, i: usize) -> String {
    format!("{}[{}]", path, i)
}

/// Last object key of a JSONPath-ish string (used to detect secrets).
pub fn last_key(path: &str) -> Option<String> {
    let p = path.trim_end_matches(']');
    let idx = p.rfind(['.', '[']).map(|i| i + 1).unwrap_or(0);
    let k = p[idx..].trim_matches(|c| c == '\'' || c == '"').to_string();
    if k.is_empty() || k.chars().all(|c| c.is_ascii_digit() || c == '*') {
        None
    } else {
        Some(k)
    }
}

/// Convert a JSON pointer (`/a/0/b`) into a JSONPath by walking `instance`.
pub fn pointer_to_jsonpath(pointer: &str, instance: &Value) -> String {
    let mut out = "$".to_string();
    let mut cur = Some(instance);
    for seg in pointer.split('/').skip(1) {
        let seg = seg.replace("~1", "/").replace("~0", "~");
        match cur {
            Some(Value::Array(a)) => {
                let i: usize = seg.parse().unwrap_or(0);
                out = index(&out, i);
                cur = a.get(i);
            }
            Some(Value::Object(m)) => {
                out = child(&out, &seg);
                cur = m.get(&seg);
            }
            _ => out = child(&out, &seg),
        }
    }
    out
}

// ------------------------------------------------------------- compare ---

/// Equality that treats `1` and `1.0` as equal.
pub fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

#[derive(Debug, Clone)]
pub struct Diff {
    pub path: String,
    pub expected: Value,
    pub actual: Value,
    pub note: &'static str,
}

/// Differences between `expected` and `actual`, depth first, in document order.
pub fn diff(expected: &Value, actual: &Value) -> Vec<Diff> {
    let mut out = Vec::new();
    diff_at("$", expected, actual, &mut out);
    out
}

fn diff_at(path: &str, e: &Value, a: &Value, out: &mut Vec<Diff>) {
    let missing = |p: String, ev: &Value| Diff {
        path: p,
        expected: ev.clone(),
        actual: Value::Null,
        note: "missing in actual",
    };
    let unexpected = |p: String, av: &Value| Diff {
        path: p,
        expected: Value::Null,
        actual: av.clone(),
        note: "unexpected in actual",
    };
    match (e, a) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, ev) in x {
                let p = child(path, k);
                match y.get(k) {
                    Some(av) => diff_at(&p, ev, av, out),
                    None => out.push(missing(p, ev)),
                }
            }
            for (k, av) in y {
                if !x.contains_key(k) {
                    out.push(unexpected(child(path, k), av));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            for i in 0..x.len().max(y.len()) {
                let p = index(path, i);
                match (x.get(i), y.get(i)) {
                    (Some(ev), Some(av)) => diff_at(&p, ev, av, out),
                    (Some(ev), None) => out.push(missing(p, ev)),
                    (None, Some(av)) => out.push(unexpected(p, av)),
                    (None, None) => {}
                }
            }
        }
        _ => {
            if !json_eq(e, a) {
                out.push(Diff {
                    path: path.to_string(),
                    expected: e.clone(),
                    actual: a.clone(),
                    note: "value differs",
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn removed(v: Value, p: &str) -> Value {
        let mut v = v;
        assert!(modify(&mut v, p, &Op::Remove));
        v
    }

    #[test]
    fn removes_simple_paths() {
        assert_eq!(
            removed(json!({"a": {"b": 1, "c": 2}}), "$.a.b"),
            json!({"a": {"c": 2}})
        );
        assert_eq!(
            removed(json!({"a": [{"b": 1}, {"b": 2}]}), "$.a[0].b"),
            json!({"a": [{}, {"b": 2}]})
        );
        assert_eq!(
            removed(
                json!({"items": [{"id": 1, "n": "x"}, {"id": 2, "n": "y"}]}),
                "$.items[*].id"
            ),
            json!({"items": [{"n": "x"}, {"n": "y"}]})
        );
        assert_eq!(
            removed(
                json!({"id": 1, "u": {"id": 2, "x": [{"id": 3, "k": 1}]}}),
                "$..id"
            ),
            json!({"u": {"x": [{"k": 1}]}})
        );
        assert_eq!(
            removed(json!({"a b": 1, "c": 2}), "$['a b']"),
            json!({"c": 2})
        );
    }

    #[test]
    fn replaces_and_rejects_unsupported() {
        let mut v = json!({"a": {"id": 1}, "id": 2});
        assert!(modify(&mut v, "$..id", &Op::Replace(json!("<ignored>"))));
        assert_eq!(v, json!({"a": {"id": "<ignored>"}, "id": "<ignored>"}));
        assert!(!modify(&mut v, "$.a[?(@.x)]", &Op::Remove));
    }

    #[test]
    fn diffs_report_paths() {
        let d = diff(
            &json!({"a": [1, {"b": 2}], "c": 1}),
            &json!({"a": [1, {"b": 3}], "d": 1}),
        );
        let paths: Vec<_> = d.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, vec!["$.a[1].b", "$.c", "$.d"]);
        assert!(json_eq(&json!(1), &json!(1.0)));
    }

    #[test]
    fn pointer_conversion() {
        let inst = json!({"items": [{"name": "x"}]});
        assert_eq!(
            pointer_to_jsonpath("/items/0/name", &inst),
            "$.items[0].name"
        );
        assert_eq!(pointer_to_jsonpath("", &inst), "$");
        assert_eq!(last_key("$.a.token").as_deref(), Some("token"));
        assert_eq!(last_key("$.a[0]"), None);
    }
}
