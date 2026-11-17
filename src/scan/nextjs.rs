//! Next.js scanner: App Router `app/**/route.ts` and Pages Router
//! `pages/api/**`.
//!
//! A static string-literal `basePath` from the `next.config.*` of the app
//! owning a route file is prepended to its path. Limitations: `middleware`
//! rewrites, `next.config` rewrites and i18n prefixes are not applied. Pages Router methods are detected from
//! `req.method` comparisons / `switch`; when none is found the route is GET.

use super::ts_common::{
    collect_query, dedup, is_ts_source, line_of, rel_path, str_lit, txt, unwrap, Parsers,
    HTTP_METHODS,
};
use super::Route;
use crate::naming;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tree_sitter::Node;

pub fn scan(root: &Path, files: &[PathBuf]) -> Result<Vec<Route>> {
    Ok(scan_with_warnings(root, files)?.0)
}

pub fn scan_with_warnings(root: &Path, files: &[PathBuf]) -> Result<(Vec<Route>, Vec<String>)> {
    let mut parsers = Parsers::new()?;
    let mut routes = Vec::new();
    let mut warnings = Vec::new();
    let mut owners = Owners::new(root);
    // App roots (owner dirs) that contributed at least one route, in scan order.
    let mut app_roots: Vec<PathBuf> = Vec::new();
    let mut catch_all: Vec<String> = Vec::new();
    for f in files.iter().filter(|f| is_ts_source(f) && may_be_route(f)) {
        let rel = rel_path(root, f);
        let owner = owners.owner_of(f);
        // Locate `app/` / `pages/api/` relative to the owning app, so that a
        // directory named `app` above the project cannot confuse the scan.
        let in_app = rel_path(&owner, f);
        let comps: Vec<&str> = in_app.split('/').collect();
        let Some((file, dirs)) = comps.split_last() else {
            continue;
        };
        let stem = file.rsplit_once('.').map(|x| x.0).unwrap_or(file);
        let app_idx = dirs.iter().position(|d| *d == "app");
        let pages_idx = dirs
            .windows(2)
            .position(|w| w[0] == "pages" && w[1] == "api");
        let kind = if let (true, Some(i)) = (stem == "route", app_idx) {
            Kind::App(i)
        } else if let Some(i) = pages_idx {
            Kind::Pages(i)
        } else {
            continue;
        };
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        let Some(tree) = parsers.parse(f, &src) else {
            continue;
        };
        let found = match kind {
            Kind::App(i) => {
                let Some(path) = app_path(&dirs[i + 1..]) else {
                    continue;
                };
                if dirs[i + 1..].iter().any(|d| is_catch_all(d)) {
                    catch_all.push(path.clone());
                }
                let path = owners.prefixed(&owner, &path, &mut warnings);
                app_routes(tree.root_node(), &src, &path, &rel)
            }
            Kind::Pages(i) => {
                let mut segs: Vec<&str> = dirs[i + 2..].to_vec();
                if stem != "index" {
                    segs.push(stem);
                }
                let path = naming::normalize_path(&format!("/api/{}", segs.join("/")));
                if segs.iter().any(|d| is_catch_all(d)) {
                    catch_all.push(path.clone());
                }
                let path = owners.prefixed(&owner, &path, &mut warnings);
                pages_routes(tree.root_node(), &src, &path, &rel)
            }
        };
        if !found.is_empty() && !app_roots.contains(&owner) {
            app_roots.push(owner);
        }
        routes.extend(found);
    }
    if app_roots.len() > 1 {
        let names: Vec<String> = app_roots
            .iter()
            .map(|d| {
                let r = rel_path(root, d);
                if r.is_empty() {
                    ".".to_string()
                } else {
                    r
                }
            })
            .collect();
        warnings.push(format!(
            "found {} Next.js apps in one scan ({}): their routes share one set of endpoint files, \
             and routes with the same method and path collide. Use one ajiro-api.yaml (or one `api_dir`) \
             per app, with `sources` pointing at that app only",
            app_roots.len(),
            names.join(", ")
        ));
    }
    if !catch_all.is_empty() {
        catch_all.sort();
        catch_all.dedup();
        let shown: Vec<&str> = catch_all.iter().take(5).map(String::as_str).collect();
        warnings.push(format!(
            "{} catch-all route(s) (`[...name]` / `[[...name]]`) are listed as a single `{{name}}` \
             path but match any number of path segments (`[[...name]]` also matches none): {}{}",
            catch_all.len(),
            shown.join(", "),
            if catch_all.len() > shown.len() {
                ", ..."
            } else {
                ""
            }
        ));
    }
    Ok((routes, warnings))
}

/// Cheap pre-filter before the owner lookup (which touches the file system):
/// only `app/**/route.*` and `pages/api/**` files can define routes.
fn may_be_route(f: &Path) -> bool {
    let comps: Vec<&str> = f
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let Some((file, dirs)) = comps.split_last() else {
        return false;
    };
    let stem = file.rsplit_once('.').map(|x| x.0).unwrap_or(file);
    (stem == "route" && dirs.contains(&"app"))
        || dirs.windows(2).any(|w| w[0] == "pages" && w[1] == "api")
}

fn is_catch_all(seg: &str) -> bool {
    seg.starts_with("[...") || seg.starts_with("[[...")
}

const CONFIG_NAMES: &[&str] = &[
    "next.config.js",
    "next.config.mjs",
    "next.config.ts",
    "next.config.cjs",
    "next.config.mts",
    "next.config.cts",
];

/// Resolves the Next.js app (package) owning a file and its `basePath`.
struct Owners<'a> {
    root: &'a Path,
    by_dir: HashMap<PathBuf, PathBuf>,
    base_paths: HashMap<PathBuf, String>,
    /// Per directory: (has a `next.config.*`, has a `package.json`), so each
    /// directory is probed once (stat is slow on network / WSL mounts).
    markers: HashMap<PathBuf, (bool, bool)>,
}

impl<'a> Owners<'a> {
    fn new(root: &'a Path) -> Self {
        Owners {
            root,
            by_dir: HashMap::new(),
            base_paths: HashMap::new(),
            markers: HashMap::new(),
        }
    }

    /// Nearest ancestor with a `next.config.*`, else nearest with a
    /// `package.json`, else the project root. Never looks above the project
    /// root for files inside it.
    fn owner_of(&mut self, file: &Path) -> PathBuf {
        let dir = file.parent().unwrap_or(self.root).to_path_buf();
        if let Some(o) = self.by_dir.get(&dir) {
            return o.clone();
        }
        let inside = dir.starts_with(self.root);
        let mut pkg: Option<PathBuf> = None;
        let mut found: Option<PathBuf> = None;
        for a in dir.ancestors() {
            let (has_config, has_pkg) = *self
                .markers
                .entry(a.to_path_buf())
                .or_insert_with(|| dir_markers(a));
            if has_config {
                found = Some(a.to_path_buf());
                break;
            }
            if pkg.is_none() && has_pkg {
                pkg = Some(a.to_path_buf());
            }
            if inside && a == self.root {
                break;
            }
        }
        let owner = found.or(pkg).unwrap_or_else(|| self.root.to_path_buf());
        self.by_dir.insert(dir, owner.clone());
        owner
    }

    /// `path` with the owner's static `basePath` prepended.
    fn prefixed(&mut self, owner: &Path, path: &str, warnings: &mut Vec<String>) -> String {
        if !self.base_paths.contains_key(owner) {
            let bp = read_base_path(self.root, owner, warnings);
            self.base_paths.insert(owner.to_path_buf(), bp);
        }
        let bp = &self.base_paths[owner];
        if bp.is_empty() {
            path.to_string()
        } else {
            naming::normalize_path(&format!("{}{}", bp, path))
        }
    }
}

/// (has a `next.config.*`, has a `package.json`) for `dir`, from one directory
/// listing instead of seven `stat` calls.
fn dir_markers(dir: &Path) -> (bool, bool) {
    let names = super::dircache::names(dir);
    (
        CONFIG_NAMES.iter().any(|n| names.contains(*n)),
        names.contains("package.json"),
    )
}

/// Static string-literal `basePath` of the `next.config.*` in `owner`
/// (`""` when absent or not a literal; the latter also warns).
fn read_base_path(root: &Path, owner: &Path, warnings: &mut Vec<String>) -> String {
    for name in CONFIG_NAMES {
        let cfg = owner.join(name);
        let Ok(text) = std::fs::read_to_string(&cfg) else {
            continue;
        };
        return match parse_base_path(&text) {
            BasePath::None => String::new(),
            BasePath::Literal(v) => {
                let v = v.trim().trim_matches('/');
                if v.is_empty() {
                    String::new()
                } else {
                    format!("/{}", v)
                }
            }
            BasePath::Dynamic => {
                warnings.push(format!(
                    "{}: `basePath` is not a plain string literal, so routes are listed without it \
                     (use a literal, or add the prefix to the endpoint paths by hand)",
                    rel_path(root, &cfg)
                ));
                String::new()
            }
        };
    }
    String::new()
}

#[derive(Debug, PartialEq)]
enum BasePath {
    None,
    Literal(String),
    Dynamic,
}

fn parse_base_path(text: &str) -> BasePath {
    let mut mentioned = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            continue;
        }
        let Some(i) = find_word(t, "basePath") else {
            continue;
        };
        mentioned = true;
        let rest = t[i + "basePath".len()..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(q) = rest
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\'' | '`'))
        else {
            return BasePath::Dynamic;
        };
        let body = &rest[1..];
        let Some(end) = body.find(q) else {
            return BasePath::Dynamic;
        };
        let after = body[end + 1..].trim_start();
        let simple = after.is_empty()
            || after.starts_with(',')
            || after.starts_with('}')
            || after.starts_with("//");
        if !simple || (q == '`' && body[..end].contains("${")) {
            return BasePath::Dynamic;
        }
        return BasePath::Literal(body[..end].to_string());
    }
    if mentioned {
        BasePath::Dynamic
    } else {
        BasePath::None
    }
}

fn find_word(line: &str, word: &str) -> Option<usize> {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    line.match_indices(word).map(|(i, _)| i).find(|&i| {
        !line[..i].chars().next_back().is_some_and(ident)
            && !line[i + word.len()..].chars().next().is_some_and(ident)
    })
}

enum Kind {
    App(usize),
    Pages(usize),
}

/// URL path of an App Router directory chain; `None` for private / intercepting routes.
fn app_path(dirs: &[&str]) -> Option<String> {
    let mut segs = Vec::new();
    for d in dirs {
        if d.starts_with('_') || d.starts_with("(.") {
            return None;
        }
        if d.starts_with('(') && d.ends_with(')') || d.starts_with('@') {
            continue;
        }
        segs.push(*d);
    }
    Some(naming::normalize_path(&segs.join("/")))
}

fn app_routes(root: Node, src: &str, path: &str, rel: &str) -> Vec<Route> {
    let mut found: Vec<(String, u32, Option<String>)> = Vec::new();
    let mut add = |name: &str, line: u32, handler: Option<String>| {
        if HTTP_METHODS.contains(&name) && !found.iter().any(|f| f.0 == name) {
            found.push((name.to_string(), line, handler));
        }
    };
    let mut c = root.walk();
    for st in root
        .named_children(&mut c)
        .filter(|n| n.kind() == "export_statement")
    {
        let has_source = st.child_by_field_name("source").is_some();
        if let Some(d) = st.child_by_field_name("declaration") {
            match d.kind() {
                "function_declaration" | "generator_function_declaration" => {
                    if let Some(n) = d.child_by_field_name("name") {
                        add(txt(n, src), line_of(d), Some(txt(n, src).to_string()));
                    }
                }
                "lexical_declaration" | "variable_declaration" => {
                    let mut c2 = d.walk();
                    for decl in d
                        .named_children(&mut c2)
                        .filter(|x| x.kind() == "variable_declarator")
                    {
                        let Some(name) = decl.child_by_field_name("name") else {
                            continue;
                        };
                        match name.kind() {
                            "identifier" => add(
                                txt(name, src),
                                line_of(decl),
                                Some(txt(name, src).to_string()),
                            ),
                            "object_pattern" => {
                                let mut c3 = name.walk();
                                for p in name.named_children(&mut c3) {
                                    if p.kind() == "shorthand_property_identifier_pattern" {
                                        add(txt(p, src), line_of(decl), None);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let mut c2 = st.walk();
        for clause in st.children(&mut c2).filter(|x| x.kind() == "export_clause") {
            let mut c3 = clause.walk();
            for sp in clause
                .named_children(&mut c3)
                .filter(|x| x.kind() == "export_specifier")
            {
                let Some(name) = sp.child_by_field_name("name") else {
                    continue;
                };
                let exported = sp.child_by_field_name("alias").unwrap_or(name);
                let handler =
                    (!has_source && exported.id() != name.id()).then(|| txt(name, src).to_string());
                add(txt(exported, src), line_of(sp), handler);
            }
        }
    }
    let mut query = Vec::new();
    collect_query(root, src, &mut query);
    dedup(&mut query);
    let has_get = found.iter().any(|f| f.0 == "GET");
    found
        .into_iter()
        .map(|(method, line, handler)| {
            let q = if method == "GET" || !has_get {
                query.clone()
            } else {
                vec![]
            };
            Route {
                method,
                path: path.to_string(),
                file: rel.to_string(),
                line,
                handler,
                query_params: q,
                framework: "nextjs".to_string(),
            }
        })
        .collect()
}

fn pages_routes(root: Node, src: &str, path: &str, rel: &str) -> Vec<Route> {
    // Default export -> line + handler name.
    let mut line = 1;
    let mut handler = None;
    let mut c = root.walk();
    for st in root
        .named_children(&mut c)
        .filter(|n| n.kind() == "export_statement")
    {
        let is_default = {
            let mut c2 = st.walk();
            let r = st.children(&mut c2).any(|x| x.kind() == "default");
            r
        };
        if !is_default {
            continue;
        }
        line = line_of(st);
        if let Some(d) = st.child_by_field_name("declaration") {
            handler = d
                .child_by_field_name("name")
                .map(|n| txt(n, src).to_string());
        } else if let Some(v) = st.child_by_field_name("value") {
            let v = unwrap(v);
            if v.kind() == "identifier" {
                handler = Some(txt(v, src).to_string());
            }
        }
        break;
    }
    let mut methods = Vec::new();
    find_methods(root, src, &mut methods);
    dedup(&mut methods);
    methods.retain(|m| HTTP_METHODS.contains(&m.as_str()));
    if methods.is_empty() {
        methods.push("GET".to_string());
    }
    let mut query = Vec::new();
    collect_query(root, src, &mut query);
    dedup(&mut query);
    methods
        .into_iter()
        .map(|method| Route {
            method,
            path: path.to_string(),
            file: rel.to_string(),
            line,
            handler: handler.clone(),
            query_params: query.clone(),
            framework: "nextjs".to_string(),
        })
        .collect()
}

fn is_method_expr(n: Node, src: &str) -> bool {
    let n = unwrap(n);
    match n.kind() {
        "member_expression" => {
            n.child_by_field_name("property").map(|p| txt(p, src)) == Some("method")
        }
        "identifier" => txt(n, src) == "method",
        _ => false,
    }
}

/// `req.method === 'POST'`, `'POST' !== req.method`, `switch (req.method) { case 'POST': }`.
fn find_methods(n: Node, src: &str, out: &mut Vec<String>) {
    match n.kind() {
        "binary_expression" => {
            let op = n
                .child_by_field_name("operator")
                .map(|o| txt(o, src))
                .unwrap_or("");
            if matches!(op, "===" | "==" | "!==" | "!=") {
                if let (Some(l), Some(r)) = (
                    n.child_by_field_name("left"),
                    n.child_by_field_name("right"),
                ) {
                    let lit = if is_method_expr(l, src) {
                        str_lit(r, src)
                    } else if is_method_expr(r, src) {
                        str_lit(l, src)
                    } else {
                        None
                    };
                    if let Some(s) = lit {
                        out.push(s.to_ascii_uppercase());
                    }
                }
            }
        }
        "switch_statement" => {
            if let (Some(v), Some(body)) = (
                n.child_by_field_name("value"),
                n.child_by_field_name("body"),
            ) {
                if is_method_expr(v, src) {
                    let mut c = body.walk();
                    for case in body
                        .named_children(&mut c)
                        .filter(|x| x.kind() == "switch_case")
                    {
                        if let Some(s) = case
                            .child_by_field_name("value")
                            .and_then(|v| str_lit(v, src))
                        {
                            out.push(s.to_ascii_uppercase());
                        }
                    }
                }
            }
        }
        _ => {}
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        find_methods(ch, src, out);
    }
}
