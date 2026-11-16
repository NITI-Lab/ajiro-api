//! Next.js scanner: App Router `app/**/route.ts` and Pages Router
//! `pages/api/**`.
//!
//! Limitations: `middleware` rewrites, `next.config` rewrites/basePath and
//! i18n prefixes are not applied. Pages Router methods are detected from
//! `req.method` comparisons / `switch`; when none is found the route is GET.

use super::ts_common::{
    collect_query, dedup, is_ts_source, line_of, rel_path, str_lit, txt, unwrap, Parsers,
    HTTP_METHODS,
};
use super::Route;
use crate::naming;
use anyhow::Result;
use std::path::{Path, PathBuf};
use tree_sitter::Node;

pub fn scan(root: &Path, files: &[PathBuf]) -> Result<Vec<Route>> {
    Ok(scan_with_warnings(root, files)?.0)
}

pub fn scan_with_warnings(root: &Path, files: &[PathBuf]) -> Result<(Vec<Route>, Vec<String>)> {
    let mut parsers = Parsers::new()?;
    let mut routes = Vec::new();
    for f in files.iter().filter(|f| is_ts_source(f)) {
        let rel = rel_path(root, f);
        let comps: Vec<&str> = rel.split('/').collect();
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
        match kind {
            Kind::App(i) => {
                let Some(path) = app_path(&dirs[i + 1..]) else {
                    continue;
                };
                routes.extend(app_routes(tree.root_node(), &src, &path, &rel));
            }
            Kind::Pages(i) => {
                let mut segs: Vec<&str> = dirs[i + 2..].to_vec();
                if stem != "index" {
                    segs.push(stem);
                }
                let path = naming::normalize_path(&format!("/api/{}", segs.join("/")));
                routes.extend(pages_routes(tree.root_node(), &src, &path, &rel));
            }
        }
    }
    Ok((routes, vec![]))
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
