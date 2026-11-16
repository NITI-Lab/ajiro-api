//! Shared naming rules: path normalization, endpoint ids and file locations.
//! Used by `scan`, the OpenAPI/Postman/Bruno importers and the mock server so
//! that every producer of endpoint files agrees on names.

use std::path::{Path, PathBuf};

/// Convert framework-specific path syntax into the OpenAPI style used in YAML.
///
/// - `:id` (Express, Hono, Echo) -> `{id}`; `:id?` -> `{id}`
/// - `[id]`, `[...slug]`, `[[...slug]]` (Next.js) -> `{id}`, `{slug}`
/// - `{id...}`, `{id:[0-9]+}` (Go 1.22 / chi) -> `{id}`
/// - `{id:int}`, `{path:path}` (FastAPI/Starlette converters) -> `{id}`, `{path}`
/// - `*` -> `{wildcard}`
///
/// Also collapses duplicate slashes, ensures a leading `/` and removes a
/// trailing `/` (except for the root path).
pub fn normalize_path(raw: &str) -> String {
    let mut segs = Vec::new();
    for seg in raw.split('/') {
        if seg.is_empty() {
            continue;
        }
        segs.push(normalize_segment(seg));
    }
    if segs.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segs.join("/"))
    }
}

fn normalize_segment(seg: &str) -> String {
    if let Some(name) = seg.strip_prefix(':') {
        let name = name.trim_end_matches('?');
        // Hono allows regex constraints: `:id{[0-9]+}`
        let name = name.split('{').next().unwrap_or(name);
        return format!("{{{}}}", name);
    }
    if seg.starts_with('[') && seg.ends_with(']') {
        let inner = seg.trim_start_matches('[').trim_end_matches(']');
        let inner = inner.trim_start_matches("...");
        return format!("{{{}}}", inner);
    }
    if seg.starts_with('{') && seg.ends_with('}') {
        let inner = &seg[1..seg.len() - 1];
        let name = inner
            .split([':', '.'])
            .next()
            .unwrap_or(inner);
        return format!("{{{}}}", name);
    }
    if seg == "*" {
        return "{wildcard}".to_string();
    }
    seg.to_string()
}

/// Parameter names in an OpenAPI-style path, in order.
pub fn path_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| s.starts_with('{') && s.ends_with('}'))
        .map(|s| s[1..s.len() - 1].to_string())
        .collect()
}

fn is_param(seg: &str) -> bool {
    seg.starts_with('{') && seg.ends_with('}')
}

fn is_prefix_segment(seg: &str) -> bool {
    seg == "api" || (seg.len() >= 2 && seg.starts_with('v') && seg[1..].chars().all(|c| c.is_ascii_digit()))
}

/// Static (non-parameter) segments, without `api` / `v1`-style prefixes.
fn meaningful_segments(path: &str) -> Vec<String> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut start = 0;
    while start < segs.len() && is_prefix_segment(segs[start]) {
        start += 1;
    }
    segs[start..]
        .iter()
        .filter(|s| !is_param(s))
        .map(|s| sanitize(s))
        .filter(|s| !s.is_empty())
        .collect()
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Resource directory name: first meaningful segment, or `root`.
pub fn resource_of(path: &str) -> String {
    meaningful_segments(path)
        .into_iter()
        .next()
        .unwrap_or_else(|| "root".to_string())
}

/// File stem: `<method>-<segments>`, parameters included by name.
/// `POST /users` -> `post-users`, `GET /users/{id}` -> `get-users-id`,
/// `GET /` -> `get-root`.
pub fn file_stem(method: &str, path: &str) -> String {
    let parts: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| if is_param(s) { sanitize(&s[1..s.len() - 1]) } else { sanitize(s) })
        .filter(|s| !s.is_empty())
        .collect();
    let tail = if parts.is_empty() { "root".to_string() } else { parts.join("-") };
    format!("{}-{}", method.to_ascii_lowercase(), tail)
}

/// Relative file path of an endpoint inside `api_dir`:
/// `<resource>/<method>-<path>.yaml`.
pub fn endpoint_rel_path(method: &str, path: &str) -> PathBuf {
    Path::new(&resource_of(path)).join(format!("{}.yaml", file_stem(method, path)))
}

/// Default endpoint id: `<segments joined by .>.<action>`.
///
/// - `GET /users` -> `users.list`, `GET /users/{id}` -> `users.get`
/// - `POST /users` -> `users.create`, `PUT` -> `replace`, `PATCH` -> `update`,
///   `DELETE` -> `delete`, other methods -> lower-case method name.
/// - `GET /users/{id}/posts` -> `users.posts.list`
pub fn endpoint_id(method: &str, path: &str) -> String {
    let segs = meaningful_segments(path);
    let base = if segs.is_empty() { "root".to_string() } else { segs.join(".") };
    let ends_with_param = path
        .split('/')
        .rfind(|s| !s.is_empty())
        .map(is_param)
        .unwrap_or(false);
    let m = method.to_ascii_uppercase();
    let action = match m.as_str() {
        "GET" if ends_with_param => "get".to_string(),
        "GET" => "list".to_string(),
        "POST" => "create".to_string(),
        "PUT" => "replace".to_string(),
        "PATCH" => "update".to_string(),
        "DELETE" => "delete".to_string(),
        other => other.to_ascii_lowercase(),
    };
    format!("{}.{}", base, action)
}

/// Make `id` unique against `taken` by appending `-2`, `-3`, ...
pub fn unique_id(id: &str, taken: &std::collections::HashSet<String>) -> String {
    if !taken.contains(id) {
        return id.to_string();
    }
    let mut n = 2;
    loop {
        let cand = format!("{}-{}", id, n);
        if !taken.contains(&cand) {
            return cand;
        }
        n += 1;
    }
}

/// Slug used for snapshot file names and JUnit test names.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "test".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_framework_syntax() {
        assert_eq!(normalize_path("/users/:id"), "/users/{id}");
        assert_eq!(normalize_path("/users/:id{[0-9]+}"), "/users/{id}");
        assert_eq!(normalize_path("users/[id]/"), "/users/{id}");
        assert_eq!(normalize_path("/docs/[...slug]"), "/docs/{slug}");
        assert_eq!(normalize_path("/files/{path:path}"), "/files/{path}");
        assert_eq!(normalize_path("/items/{id...}"), "/items/{id}");
        assert_eq!(normalize_path("//a//b/"), "/a/b");
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("/static/*"), "/static/{wildcard}");
    }

    #[test]
    fn names() {
        assert_eq!(endpoint_rel_path("POST", "/users"), Path::new("users/post-users.yaml"));
        assert_eq!(endpoint_rel_path("GET", "/api/v1/users/{id}"), Path::new("users/get-api-v1-users-id.yaml"));
        assert_eq!(endpoint_rel_path("GET", "/"), Path::new("root/get-root.yaml"));
        assert_eq!(endpoint_id("POST", "/users"), "users.create");
        assert_eq!(endpoint_id("GET", "/users"), "users.list");
        assert_eq!(endpoint_id("GET", "/api/users/{id}"), "users.get");
        assert_eq!(endpoint_id("GET", "/users/{id}/posts"), "users.posts.list");
        assert_eq!(endpoint_id("POST", "/auth/login"), "auth.login.create");
        assert_eq!(path_params("/a/{x}/b/{y}"), vec!["x", "y"]);
        assert_eq!(slug("メール重複は409"), "メール重複は409");
        assert_eq!(slug("Creates a user!"), "creates-a-user");
    }
}
