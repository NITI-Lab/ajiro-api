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
    normalize_path_checked(raw).0
}

/// Placeholder name for a parameter whose name cannot be recovered.
pub const UNNAMED_PARAM: &str = "param";

/// Like [`normalize_path`], and also reports whether any parameter had no
/// recoverable name (it became `{param}`). The scanner turns that into a
/// warning that quotes the raw path.
pub fn normalize_path_checked(raw: &str) -> (String, bool) {
    let mut segs = Vec::new();
    let mut unnamed = false;
    for seg in split_segments(raw) {
        if seg.is_empty() {
            continue;
        }
        segs.push(normalize_segment(seg, &mut unnamed));
    }
    let path = if segs.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segs.join("/"))
    };
    (path, unnamed)
}

/// Split on `/`, except inside `{...}` (Hono regex constraints such as
/// `:id{[^/]+}` may contain slashes).
fn split_segments(raw: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in raw.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '/' if depth == 0 => {
                out.push(&raw[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&raw[start..]);
    out
}

fn param(name: &str, unnamed: &mut bool) -> String {
    let name = name.trim();
    if name.is_empty() {
        *unnamed = true;
        format!("{{{}}}", UNNAMED_PARAM)
    } else {
        format!("{{{}}}", name)
    }
}

fn normalize_segment(seg: &str, unnamed: &mut bool) -> String {
    if let Some(name) = seg.strip_prefix(':') {
        let name = name.trim_end_matches('?');
        // Hono allows regex constraints: `:id{[0-9]+}`
        let name = name.split('{').next().unwrap_or(name);
        return param(name, unnamed);
    }
    if seg.starts_with('[') && seg.ends_with(']') {
        let inner = seg.trim_start_matches('[').trim_end_matches(']');
        let inner = inner.trim_start_matches("...");
        return param(inner, unnamed);
    }
    if seg.starts_with('{') && seg.ends_with('}') {
        let inner = &seg[1..seg.len() - 1];
        // `{id:[0-9]+}`, `{id...}`; a stray `{:id}` keeps the name after the colon.
        let name = match inner.strip_prefix(':') {
            Some(rest) if rest.starts_with(|c: char| c.is_alphabetic() || c == '_') => {
                rest.split([':', '.', '{', '?']).next().unwrap_or("")
            }
            Some(_) => "",
            None => inner.split([':', '.', '{']).next().unwrap_or(inner),
        };
        return param(name, unnamed);
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
    seg.len() >= 2
        && seg.starts_with('{')
        && seg.ends_with('}')
        && !seg[1..seg.len() - 1].contains(['{', '}'])
}

/// A segment mixing parameters and literal text, e.g. `{item_id}.png`.
fn is_mixed(seg: &str) -> bool {
    !is_param(seg) && (seg.contains('{') || seg.contains('['))
}

/// Literal text of a mixed segment with every `{...}` / `[...]` group removed
/// (`{item_id}.png` -> `.png`).
fn literal_part(seg: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in seg.chars() {
        match c {
            '{' | '[' => depth += 1,
            '}' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// A segment that carries a parameter (whole or mixed).
fn has_param(seg: &str) -> bool {
    is_param(seg) || is_mixed(seg)
}

fn is_prefix_segment(seg: &str) -> bool {
    seg == "api"
        || (seg.len() >= 2 && seg.starts_with('v') && seg[1..].chars().all(|c| c.is_ascii_digit()))
}

/// Static (non-parameter) segments, without `api` / `v1`-style prefixes.
///
/// A path made only of parameters after the prefix (a catch-all such as
/// `/api/v1/{resource}/{id}`) has no static segment; it falls back to the
/// parameter names so that `GET /{resource}` and `GET /{resource}/{id}` get
/// distinct, readable names instead of `root.get` and `root.get-2`.
fn meaningful_segments(path: &str) -> Vec<String> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut start = 0;
    while start < segs.len() && is_prefix_segment(segs[start]) {
        start += 1;
    }
    let statics: Vec<String> = segs[start..]
        .iter()
        .filter(|s| !is_param(s))
        .map(|s| {
            if is_mixed(s) {
                sanitize(&literal_part(s))
            } else {
                sanitize(s)
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    if !statics.is_empty() {
        return statics;
    }
    segs[start..]
        .iter()
        .filter(|s| is_param(s))
        .map(|s| sanitize(&s[1..s.len() - 1]))
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
        .map(|s| {
            if is_param(s) {
                sanitize(&s[1..s.len() - 1])
            } else {
                sanitize(s)
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    let tail = if parts.is_empty() {
        "root".to_string()
    } else {
        parts.join("-")
    };
    format!("{}-{}", method.to_ascii_lowercase(), tail)
}

/// Relative file path of an endpoint inside `api_dir`:
/// `<resource>/<method>-<path>.yaml`.
pub fn endpoint_rel_path(method: &str, path: &str) -> PathBuf {
    Path::new(&resource_of(path)).join(format!("{}.yaml", file_stem(method, path)))
}

/// Collection-like words that make a trailing static segment a `list`.
const COLLECTION_WORDS: &[&str] = &[
    "list", "all", "search", "history", "index", "recent", "feed", "news", "overview", "summary",
    "children", "people", "data",
];

/// Does a trailing static segment name a collection (`users`, `logs`,
/// `search`) rather than a single resource (`health`, `me`, `detail`)?
///
/// Looks at the last `-`-separated word: it is a collection when it is a
/// known collection word, or ends with `s` but not `ss` / `us` / `is`
/// (`status`, `address`, `analysis` are singular).
fn looks_like_collection(segment: &str) -> bool {
    let word = segment.rsplit('-').next().unwrap_or(segment);
    if COLLECTION_WORDS.contains(&word) {
        return true;
    }
    word.len() > 2
        && word.ends_with('s')
        && !word.ends_with("ss")
        && !word.ends_with("us")
        && !word.ends_with("is")
}

/// Verb synonyms: a trailing static segment equal to one of these already
/// says the action, so it is not appended again (`sessions/create` ->
/// `sessions.create`, not `sessions.create.create`).
fn action_synonyms(action: &str) -> &'static [&'static str] {
    match action {
        "create" => &["create", "new", "add"],
        "replace" => &["replace", "update", "edit"],
        "update" => &["update", "edit"],
        "delete" => &["delete", "remove"],
        "list" => &["list"],
        "get" => &["get"],
        _ => &[],
    }
}

fn action_of(method: &str, last_seg: Option<&str>) -> String {
    let ends_with_param = last_seg.map(has_param).unwrap_or(false);
    match method.to_ascii_uppercase().as_str() {
        "GET" if ends_with_param => "get".to_string(),
        "GET" => {
            let seg = last_seg.map(sanitize).unwrap_or_default();
            if seg.is_empty() || is_prefix_segment(&seg) || looks_like_collection(&seg) {
                "list".to_string()
            } else {
                "get".to_string()
            }
        }
        "POST" => "create".to_string(),
        "PUT" => "replace".to_string(),
        "PATCH" => "update".to_string(),
        "DELETE" => "delete".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

fn last_segment(path: &str) -> Option<&str> {
    path.split('/').rfind(|s| !s.is_empty())
}

/// Default endpoint id: `<segments joined by .>.<action>`.
///
/// - Segments: static segments only; `api` / `v1` prefixes and parameters are
///   dropped. A segment mixing a parameter and literal text
///   (`{item_id}.png`) keeps only its literal text (`png`) and counts as a
///   parameter for the action.
/// - Action: `GET` ending with a parameter -> `get`; other `GET` -> `list` when
///   the last segment looks like a collection (`users`, `logs`, `search`),
///   else `get` (`health`, `me`); `POST` -> `create`, `PUT` -> `replace`,
///   `PATCH` -> `update`, `DELETE` -> `delete`, other methods -> lower-case
///   method name.
/// - The action is not repeated when the last static segment already is the
///   action or a synonym (`/sessions/create` -> `sessions.create`).
pub fn endpoint_id(method: &str, path: &str) -> String {
    let segs = meaningful_segments(path);
    let action = action_of(method, last_segment(path));
    let last = last_segment(path).filter(|s| !has_param(s));
    let redundant = !segs.is_empty()
        && last.is_some()
        && segs
            .last()
            .is_some_and(|l| action_synonyms(&action).contains(&l.as_str()));
    if segs.is_empty() {
        format!("root.{}", action)
    } else if redundant {
        segs.join(".")
    } else {
        format!("{}.{}", segs.join("."), action)
    }
}

/// The disambiguated id: every segment including the `api` / `vN` prefix, with
/// parameter names at their positions, and the action always appended.
/// `GET /api/users/{id}/posts` -> `api.users.id.posts.list`.
pub fn endpoint_id_full(method: &str, path: &str) -> String {
    let mut parts: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if is_param(s) {
                sanitize(&s[1..s.len() - 1])
            } else {
                sanitize(s)
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        parts.push("root".to_string());
    }
    format!(
        "{}.{}",
        parts.join("."),
        action_of(method, last_segment(path))
    )
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

/// Assign ids to a batch of new `(method, path)` routes, independent of their
/// order, against ids already `taken` (existing files; not modified).
///
/// 1. Routes with a unique [`endpoint_id`] that is not taken keep it.
/// 2. When several routes share a base id, the "plain" one (no dropped prefix
///    and no parameter, i.e. whose [`endpoint_id_full`] equals the base) keeps
///    it; the others get [`endpoint_id_full`]
///    (`medical.offices.users.list` vs
///    `medical.offices.medical-office-id.users.list`, `health.list` vs
///    `api.health.list`). If no route is plain, all get the full form. A base
///    id that is already taken is never reused.
/// 3. Anything still identical gets `-2`, `-3`, ... in `(path, method)` order.
pub fn assign_ids(
    routes: &[(String, String)],
    taken: &std::collections::HashSet<String>,
) -> Vec<String> {
    use std::collections::{HashMap, HashSet};
    let bases: Vec<String> = routes.iter().map(|(m, p)| endpoint_id(m, p)).collect();
    let fulls: Vec<String> = routes.iter().map(|(m, p)| endpoint_id_full(m, p)).collect();
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, b) in bases.iter().enumerate() {
        groups.entry(b).or_default().push(i);
    }
    let mut chosen: Vec<Option<String>> = vec![None; routes.len()];
    let mut used: HashSet<String> = taken.clone();
    let mut rest: Vec<usize> = Vec::new();
    for (base, members) in &groups {
        let base_free = !taken.contains(*base);
        let keepers: Vec<usize> = if members.len() == 1 {
            members.clone()
        } else {
            members
                .iter()
                .copied()
                .filter(|&i| fulls[i] == **base)
                .collect()
        };
        if base_free && keepers.len() == 1 {
            let k = keepers[0];
            chosen[k] = Some(base.to_string());
            used.insert(base.to_string());
        }
        for &i in members {
            if chosen[i].is_none() {
                rest.push(i);
            }
        }
    }
    rest.sort_by(|&a, &b| (&routes[a].1, &routes[a].0, a).cmp(&(&routes[b].1, &routes[b].0, b)));
    for i in rest {
        let id = unique_id(&fulls[i], &used);
        used.insert(id.clone());
        chosen[i] = Some(id);
    }
    chosen.into_iter().map(|c| c.unwrap_or_default()).collect()
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
    if out.is_empty() {
        "test".into()
    } else {
        out
    }
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
        assert_eq!(
            endpoint_rel_path("POST", "/users"),
            Path::new("users/post-users.yaml")
        );
        assert_eq!(
            endpoint_rel_path("GET", "/api/v1/users/{id}"),
            Path::new("users/get-api-v1-users-id.yaml")
        );
        assert_eq!(
            endpoint_rel_path("GET", "/"),
            Path::new("root/get-root.yaml")
        );
        assert_eq!(endpoint_id("POST", "/users"), "users.create");
        assert_eq!(endpoint_id("GET", "/users"), "users.list");
        assert_eq!(endpoint_id("GET", "/api/users/{id}"), "users.get");
        assert_eq!(endpoint_id("GET", "/users/{id}/posts"), "users.posts.list");
        assert_eq!(endpoint_id("POST", "/auth/login"), "auth.login.create");
        // catch-all routes made only of parameters
        assert_eq!(endpoint_id("GET", "/api/v1/{resource}"), "resource.get");
        assert_eq!(
            endpoint_id("GET", "/api/v1/{resource}/{id}"),
            "resource.id.get"
        );
        assert_eq!(
            endpoint_id("PATCH", "/api/v1/{resource}/{id}"),
            "resource.id.update"
        );
        assert_eq!(
            endpoint_rel_path("GET", "/api/v1/{resource}/{id}"),
            Path::new("resource/get-api-v1-resource-id.yaml")
        );
        assert_eq!(endpoint_id("GET", "/api/v1"), "root.list");
        assert_eq!(path_params("/a/{x}/b/{y}"), vec!["x", "y"]);
        assert_eq!(slug("メール重複は409"), "メール重複は409");
        assert_eq!(slug("Creates a user!"), "creates-a-user");
    }

    #[test]
    fn collection_heuristic_for_static_get() {
        assert_eq!(endpoint_id("GET", "/users"), "users.list");
        assert_eq!(endpoint_id("GET", "/users/{id}"), "users.get");
        assert_eq!(endpoint_id("GET", "/health"), "health.get");
        assert_eq!(endpoint_id("GET", "/auth/users/me"), "auth.users.me.get");
        assert_eq!(
            endpoint_id("GET", "/candidates/detail"),
            "candidates.detail.get"
        );
        assert_eq!(endpoint_id("GET", "/rooms/stream"), "rooms.stream.get");
        assert_eq!(endpoint_id("GET", "/bimi/logo.svg"), "bimi.logo-svg.get");
        assert_eq!(
            endpoint_id("GET", "/waitlist/challenge"),
            "waitlist.challenge.get"
        );
        assert_eq!(endpoint_id("GET", "/admin/status"), "admin.status.get");
        assert_eq!(endpoint_id("GET", "/items/search"), "items.search.list");
        assert_eq!(endpoint_id("GET", "/chat/history"), "chat.history.list");
        assert_eq!(endpoint_id("GET", "/audit-logs"), "audit-logs.list");
    }

    #[test]
    fn does_not_double_the_verb() {
        assert_eq!(
            endpoint_id("POST", "/medical/offers/create"),
            "medical.offers.create"
        );
        assert_eq!(endpoint_id("DELETE", "/offers/delete"), "offers.delete");
        assert_eq!(endpoint_id("DELETE", "/offers/remove"), "offers.remove");
        assert_eq!(
            endpoint_id("GET", "/admin/quiz/units/list"),
            "admin.quiz.units.list"
        );
        assert_eq!(
            endpoint_id("GET", "/student/friend/list"),
            "student.friend.list"
        );
        assert_eq!(
            endpoint_id("POST", "/oral-exam/sessions/add"),
            "oral-exam.sessions.add"
        );
        assert_eq!(endpoint_id("PATCH", "/users/edit"), "users.edit");
        // not redundant when a parameter follows
        assert_eq!(endpoint_id("GET", "/users/list/{id}"), "users.list.get");
        assert_eq!(endpoint_id("POST", "/auth/login"), "auth.login.create");
    }

    #[test]
    fn mixed_param_and_literal_segments() {
        assert_eq!(
            endpoint_id("GET", "/w/og/{kind}/{item_id}.png"),
            "w.og.png.get"
        );
        assert_eq!(endpoint_id("GET", "/files/{name}.json"), "files.json.get");
        assert_eq!(endpoint_id("GET", "/{id}.json"), "json.get");
        assert_eq!(
            endpoint_id_full("GET", "/w/og/{kind}/{item_id}.png"),
            "w.og.kind.item-id-png.get"
        );
    }

    fn ids(routes: &[(&str, &str)], taken: &[&str]) -> Vec<String> {
        let r: Vec<(String, String)> = routes
            .iter()
            .map(|(m, p)| (m.to_string(), p.to_string()))
            .collect();
        let t = taken.iter().map(|s| s.to_string()).collect();
        assign_ids(&r, &t)
    }

    #[test]
    fn collisions_are_order_independent() {
        let a = ("GET", "/medical/offices/users");
        let b = ("GET", "/medical/offices/{medical_office_id}/users");
        let want_a = "medical.offices.users.list";
        let want_b = "medical.offices.medical-office-id.users.list";
        assert_eq!(ids(&[a, b], &[]), vec![want_a, want_b]);
        assert_eq!(ids(&[b, a], &[]), vec![want_b, want_a]);

        let h1 = ("GET", "/api/health");
        let h2 = ("GET", "/health");
        assert_eq!(ids(&[h1, h2], &[]), vec!["api.health.get", "health.get"]);
        assert_eq!(ids(&[h2, h1], &[]), vec!["health.get", "api.health.get"]);

        // no plain route: all get the full form
        assert_eq!(
            ids(&[("GET", "/api/users"), ("GET", "/v1/users")], &[]),
            vec!["api.users.list", "v1.users.list"]
        );
        // base taken by an existing file: new route is disambiguated
        assert_eq!(ids(&[h2], &["health.get"]), vec!["health.get-2"]);
        assert_eq!(ids(&[h1], &["health.get"]), vec!["api.health.get"]);
        // identical routes fall back to numeric suffixes
        assert_eq!(
            ids(&[("GET", "/users"), ("GET", "/users")], &[]),
            vec!["users.list", "users.list-2"]
        );
    }

    #[test]
    fn verb_collisions_use_full_form() {
        assert_eq!(
            ids(&[("POST", "/users/create"), ("POST", "/users")], &[]),
            vec!["users.create.create", "users.create"]
        );
    }

    #[test]
    fn never_emits_empty_param() {
        for raw in [
            "/b/:",
            "/b/:?",
            "/b/:{[0-9]+}",
            "/b/{}",
            "/b/{...}",
            "/b/{:[0-9]+}",
            "/b/[]",
            "/b/[...]",
        ] {
            let (p, unnamed) = normalize_path_checked(raw);
            assert_eq!(p, "/b/{param}", "{}", raw);
            assert!(unnamed, "{}", raw);
        }
    }

    #[test]
    fn named_params_are_not_flagged() {
        for (raw, want) in [
            ("/b/:bannerId", "/b/{bannerId}"),
            ("/b/:bannerId?", "/b/{bannerId}"),
            ("/b/:id{[0-9]+}/x", "/b/{id}/x"),
            ("/b/:id{[^/]+}", "/b/{id}"),
            ("/b/:id{[^/]+}/x", "/b/{id}/x"),
            ("/b/{:id}", "/b/{id}"),
            ("/b/{id}", "/b/{id}"),
            ("/b/{id:[0-9]+}", "/b/{id}"),
        ] {
            let (p, unnamed) = normalize_path_checked(raw);
            assert_eq!(p, want, "{}", raw);
            assert!(!unnamed, "{}", raw);
        }
    }
}
