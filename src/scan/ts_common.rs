//! Shared helpers for the TypeScript / JavaScript scanners (Next.js, Hono,
//! Express) plus the "app graph" engine used by Hono and Express: it finds
//! app / router instances, route registrations and mounts (`app.route`,
//! `app.use`) across files and composes the final paths.

use super::Route;
use crate::naming;
use anyhow::Result;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser, Tree};

pub const TS_EXTS: [&str; 8] = ["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"];
pub const HTTP_METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Whether `path` looks like a TS/JS source file worth parsing.
pub fn is_ts_source(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".d.ts") || name.contains(".test.") || name.contains(".spec.") {
        return false;
    }
    matches!(path.extension().and_then(|e| e.to_str()), Some(e) if TS_EXTS.contains(&e))
}

/// Path relative to `root`, `/`-separated.
pub fn rel_path(root: &Path, file: &Path) -> String {
    let r = file.strip_prefix(root).unwrap_or(file);
    r.to_string_lossy().replace('\\', "/")
}

pub struct Parsers {
    ts: Parser,
    tsx: Parser,
}

impl Parsers {
    pub fn new() -> Result<Self> {
        let mut ts = Parser::new();
        ts.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())?;
        let mut tsx = Parser::new();
        tsx.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())?;
        Ok(Parsers { ts, tsx })
    }

    pub fn parse(&mut self, path: &Path, src: &str) -> Option<Tree> {
        let tsx = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("tsx") | Some("jsx")
        );
        if tsx {
            self.tsx.parse(src, None)
        } else {
            self.ts.parse(src, None)
        }
    }
}

// ------------------------------------------------------------ AST helpers

pub fn txt<'a>(n: Node, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

pub fn line_of(n: Node) -> u32 {
    n.start_position().row as u32 + 1
}

/// Strip parentheses, `as`, `satisfies`, `!` and `await` wrappers.
pub fn unwrap(mut n: Node) -> Node {
    while matches!(
        n.kind(),
        "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression"
            | "await_expression"
    ) {
        match n.named_child(0) {
            Some(c) => n = c,
            None => break,
        }
    }
    n
}

/// Value of a string literal or a template literal without substitutions.
pub fn str_lit(n: Node, src: &str) -> Option<String> {
    let n = unwrap(n);
    match n.kind() {
        "string" => {
            let t = txt(n, src);
            (t.len() >= 2).then(|| t[1..t.len() - 1].to_string())
        }
        "template_string" => {
            let mut c = n.walk();
            let has_sub = n
                .children(&mut c)
                .any(|x| x.kind() == "template_substitution");
            let t = txt(n, src);
            (!has_sub && t.len() >= 2).then(|| t[1..t.len() - 1].to_string())
        }
        _ => None,
    }
}

/// Value node of `key` in an object literal.
pub fn obj_prop<'t>(obj: Node<'t>, key: &str, src: &str) -> Option<Node<'t>> {
    if obj.kind() != "object" {
        return None;
    }
    let mut c = obj.walk();
    for ch in obj.named_children(&mut c) {
        if ch.kind() == "pair" {
            if let Some(k) = ch.child_by_field_name("key") {
                if txt(k, src).trim_matches(|c| c == '"' || c == '\'') == key {
                    return ch.child_by_field_name("value");
                }
            }
        }
    }
    None
}

fn is_function_node(n: Node) -> bool {
    matches!(
        n.kind(),
        "arrow_function" | "function_expression" | "function" | "generator_function"
    )
}

pub fn dedup(v: &mut Vec<String>) {
    let mut seen = HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

/// Collect statically known query parameter names below `n`:
/// `c.req.query('x')`, `searchParams.get('x')`, `req.query.x`,
/// `req.query['x']`, `const { a, b } = req.query`.
pub fn collect_query(n: Node, src: &str, out: &mut Vec<String>) {
    match n.kind() {
        "call_expression" => {
            if let Some(f) = n.child_by_field_name("function") {
                if f.kind() == "member_expression" {
                    let prop = f
                        .child_by_field_name("property")
                        .map(|p| txt(p, src))
                        .unwrap_or("");
                    let obj = f
                        .child_by_field_name("object")
                        .map(|o| txt(o, src))
                        .unwrap_or("");
                    let hit = matches!(prop, "query" | "queries")
                        || (matches!(prop, "get" | "getAll" | "has")
                            && obj.ends_with("searchParams"));
                    if hit {
                        if let Some(a) = n
                            .child_by_field_name("arguments")
                            .and_then(|a| a.named_child(0))
                        {
                            if let Some(s) = str_lit(a, src) {
                                out.push(s);
                            }
                        }
                    }
                }
            }
        }
        "member_expression" => {
            if let (Some(obj), Some(prop)) = (
                n.child_by_field_name("object"),
                n.child_by_field_name("property"),
            ) {
                if obj.kind() == "member_expression"
                    && obj.child_by_field_name("property").map(|p| txt(p, src)) == Some("query")
                {
                    out.push(txt(prop, src).to_string());
                }
            }
        }
        "subscript_expression" => {
            if let (Some(obj), Some(idx)) = (
                n.child_by_field_name("object"),
                n.child_by_field_name("index"),
            ) {
                if obj.kind() == "member_expression"
                    && obj.child_by_field_name("property").map(|p| txt(p, src)) == Some("query")
                {
                    if let Some(s) = str_lit(idx, src) {
                        out.push(s);
                    }
                }
            }
        }
        "variable_declarator" => {
            if let (Some(name), Some(value)) = (
                n.child_by_field_name("name"),
                n.child_by_field_name("value"),
            ) {
                let v = txt(unwrap(value), src);
                if name.kind() == "object_pattern"
                    && (v.ends_with(".query") || v.ends_with("searchParams"))
                {
                    let mut c = name.walk();
                    for ch in name.named_children(&mut c) {
                        match ch.kind() {
                            "shorthand_property_identifier_pattern" => {
                                out.push(txt(ch, src).to_string())
                            }
                            "pair_pattern" => {
                                if let Some(k) = ch.child_by_field_name("key") {
                                    out.push(
                                        txt(k, src)
                                            .trim_matches(|c| c == '"' || c == '\'')
                                            .to_string(),
                                    );
                                }
                            }
                            "object_assignment_pattern" => {
                                if let Some(l) = ch.child_by_field_name("left") {
                                    out.push(txt(l, src).to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        _ => {}
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        collect_query(ch, src, out);
    }
}

// ---------------------------------------------------- module resolution

fn norm_join(dir: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        vec![]
    } else {
        dir.split('/').collect()
    };
    for seg in spec.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Find `base` (as is, with an extension, or as a directory index) in `files`.
fn lookup_file(base: &str, files: &HashMap<String, usize>) -> Option<usize> {
    if let Some(i) = files.get(base) {
        return Some(*i);
    }
    let stripped = [".js", ".mjs", ".cjs", ".jsx"]
        .iter()
        .find_map(|s| base.strip_suffix(s));
    for b in std::iter::once(base).chain(stripped) {
        for e in TS_EXTS {
            if let Some(i) = files.get(&format!("{}.{}", b, e)) {
                return Some(*i);
            }
        }
        for e in TS_EXTS {
            if let Some(i) = files.get(&format!("{}/index.{}", b, e)) {
                return Some(*i);
            }
        }
    }
    None
}

fn is_relative_spec(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".."
}

/// One `compilerOptions.paths` entry: `@/*` -> `["./src/*"]`.
struct AliasRule {
    prefix: String,
    suffix: String,
    wildcard: bool,
    /// Absolute target templates (may contain one `*`).
    targets: Vec<String>,
}

/// Import alias settings of the nearest tsconfig.json / jsconfig.json.
struct AliasCfg {
    rules: Vec<AliasRule>,
    base_url: Option<String>,
}

type PathList = Vec<(String, Vec<String>)>;

#[derive(Default)]
struct RawCfg {
    base_url: Option<PathBuf>,
    /// Directory of the config that declared `paths`, and the entries.
    paths: Option<(PathBuf, PathList)>,
}

/// Lexically normalize a path to an absolute, `/`-separated string.
fn abs_norm(p: &Path) -> String {
    // `has_root`, not `is_absolute`: values produced by this function
    // (`/C:/x/y` on Windows) are fed back in, e.g. a tsconfig `baseUrl`
    // joined with a `paths` target; they must not get the current dir again.
    let abs = if p.has_root() || p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut parts: Vec<String> = Vec::new();
    for c in abs.components() {
        match c {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::CurDir | std::path::Component::RootDir => {}
            other => parts.push(other.as_os_str().to_string_lossy().into_owned()),
        }
    }
    format!("/{}", parts.join("/")).replace('\\', "/")
}

/// Strip `//` and `/* */` comments and trailing commas (JSONC) outside strings.
fn strip_jsonc(t: &str) -> String {
    let b: Vec<char> = t.chars().collect();
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '"' {
            out.push(c);
            i += 1;
            while i < b.len() {
                out.push(b[i]);
                if b[i] == '\\' && i + 1 < b.len() {
                    out.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    break;
                }
                i += 1;
            }
            i += 1;
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == ',' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_whitespace() {
                j += 1;
            }
            if !matches!(b.get(j), Some('}') | Some(']')) {
                out.push(c);
            }
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn load_tsconfig(path: &Path, depth: u8) -> Option<RawCfg> {
    if depth > 5 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&strip_jsonc(&text)).ok()?;
    let dir = path.parent()?.to_path_buf();
    let mut cfg = RawCfg::default();
    let exts: Vec<&str> = match v.get("extends") {
        Some(serde_json::Value::String(s)) => vec![s.as_str()],
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).collect(),
        _ => vec![],
    };
    for e in exts {
        if !(e.starts_with('.') || e.starts_with('/')) {
            continue; // package extends are not followed
        }
        let mut p = dir.join(e);
        if !p.is_file() {
            p = dir.join(format!("{}.json", e));
        }
        if let Some(parent) = load_tsconfig(&p, depth + 1) {
            if parent.base_url.is_some() {
                cfg.base_url = parent.base_url;
            }
            if parent.paths.is_some() {
                cfg.paths = parent.paths;
            }
        }
    }
    if let Some(co) = v.get("compilerOptions") {
        if let Some(b) = co.get("baseUrl").and_then(|b| b.as_str()) {
            cfg.base_url = Some(PathBuf::from(abs_norm(&dir.join(b))));
        }
        if let Some(po) = co.get("paths").and_then(|p| p.as_object()) {
            let list = po
                .iter()
                .map(|(k, t)| {
                    let ts = t
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    (k.clone(), ts)
                })
                .collect();
            cfg.paths = Some((dir.clone(), list));
        }
    }
    Some(cfg)
}

fn alias_cfg_in(dir: &Path) -> Option<AliasCfg> {
    for name in ["tsconfig.json", "jsconfig.json"] {
        if !super::dircache::has(dir, name) {
            continue;
        }
        let p = dir.join(name);
        let Some(raw) = load_tsconfig(&p, 0) else {
            continue;
        };
        if raw.base_url.is_none() && raw.paths.is_none() {
            continue;
        }
        let mut rules = Vec::new();
        if let Some((pdir, list)) = &raw.paths {
            let base = raw.base_url.clone().unwrap_or_else(|| pdir.clone());
            for (pat, targets) in list {
                let (prefix, suffix, wildcard) = match pat.split_once('*') {
                    Some((a, b)) => (a.to_string(), b.to_string(), true),
                    None => (pat.clone(), String::new(), false),
                };
                rules.push(AliasRule {
                    prefix,
                    suffix,
                    wildcard,
                    targets: targets.iter().map(|t| abs_norm(&base.join(t))).collect(),
                });
            }
            rules.sort_by_key(|r| std::cmp::Reverse(r.prefix.len()));
        }
        return Some(AliasCfg {
            rules,
            base_url: raw.base_url.map(|b| abs_norm(&b)),
        });
    }
    None
}

fn alias_cfg_for(
    dir: &Path,
    cache: &mut HashMap<PathBuf, Option<Rc<AliasCfg>>>,
) -> Option<Rc<AliasCfg>> {
    if let Some(c) = cache.get(dir) {
        return c.clone();
    }
    let r = match alias_cfg_in(dir) {
        Some(c) => Some(Rc::new(c)),
        None => dir.parent().and_then(|p| alias_cfg_for(p, cache)),
    };
    cache.insert(dir.to_path_buf(), r.clone());
    r
}

/// Import resolution over the scanned files: relative specifiers,
/// tsconfig/jsconfig `paths` + `baseUrl` aliases (nearest config above the
/// importing file), and the legacy `@/` -> `src/` fallback.
struct Modules {
    rels: Vec<String>,
    by_rel: HashMap<String, usize>,
    by_full: HashMap<String, usize>,
    cfgs: Vec<Option<Rc<AliasCfg>>>,
}

impl Modules {
    fn new(paths_rels: &[(&Path, &str)]) -> Self {
        let mut cache = HashMap::new();
        let mut m = Modules {
            rels: vec![],
            by_rel: HashMap::new(),
            by_full: HashMap::new(),
            cfgs: vec![],
        };
        for (i, (p, rel)) in paths_rels.iter().enumerate() {
            m.rels.push(rel.to_string());
            m.by_rel.insert(rel.to_string(), i);
            m.by_full.insert(abs_norm(p), i);
            let cfg = p
                .parent()
                .map(|d| PathBuf::from(abs_norm(d)))
                .and_then(|d| alias_cfg_for(&d, &mut cache));
            m.cfgs.push(cfg);
        }
        m
    }

    fn alias_rule_matches(&self, fi: usize, spec: &str) -> bool {
        self.cfgs[fi].as_ref().is_some_and(|c| {
            c.rules.iter().any(|r| {
                if r.wildcard {
                    spec.len() >= r.prefix.len() + r.suffix.len()
                        && spec.starts_with(&r.prefix)
                        && spec.ends_with(&r.suffix)
                } else {
                    spec == r.prefix
                }
            })
        })
    }

    /// Whether `spec` points into the project (relative or a known alias form).
    fn is_local_spec(&self, fi: usize, spec: &str) -> bool {
        is_relative_spec(spec)
            || spec.starts_with("@/")
            || spec.starts_with("~/")
            || self.alias_rule_matches(fi, spec)
    }

    fn resolve(&self, fi: usize, spec: &str) -> Option<usize> {
        let from_rel = &self.rels[fi];
        if is_relative_spec(spec) {
            let dir = from_rel.rsplit_once('/').map(|x| x.0).unwrap_or("");
            return lookup_file(&norm_join(dir, spec), &self.by_rel);
        }
        if let Some(cfg) = &self.cfgs[fi] {
            for r in &cfg.rules {
                let mid = if r.wildcard {
                    if spec.len() < r.prefix.len() + r.suffix.len()
                        || !spec.starts_with(&r.prefix)
                        || !spec.ends_with(&r.suffix)
                    {
                        continue;
                    }
                    &spec[r.prefix.len()..spec.len() - r.suffix.len()]
                } else if spec == r.prefix {
                    ""
                } else {
                    continue;
                };
                for t in &r.targets {
                    if let Some(i) = lookup_file(&t.replacen('*', mid, 1), &self.by_full) {
                        return Some(i);
                    }
                }
            }
            if let Some(b) = &cfg.base_url {
                if let Some(i) = lookup_file(&format!("{}/{}", b, spec), &self.by_full) {
                    return Some(i);
                }
            }
        }
        let rest = spec
            .strip_prefix("@/")
            .or_else(|| spec.strip_prefix("~/"))?;
        lookup_file(&format!("src/{}", rest), &self.by_rel)
            .or_else(|| lookup_file(rest, &self.by_rel))
    }
}

// ------------------------------------------------------------ app graph

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Hono,
    Express,
}

impl Flavor {
    fn name(self) -> &'static str {
        match self {
            Flavor::Hono => "hono",
            Flavor::Express => "express",
        }
    }
}

enum ChildRef {
    Name(String),
    Key(String),
    Require(String, Option<String>),
    /// `ns.router` where `ns` is a namespace import.
    NsMember(String, String),
    /// Call of a factory function: the instances it returns.
    Factory(String),
}

struct Mount {
    parent: String,
    prefix: String,
    child: ChildRef,
    line: u32,
}

struct Reg {
    inst: String,
    method: String,
    path: String,
    line: u32,
    handler: Option<String>,
    query: Vec<String>,
    /// Identifies the registration call (shared by all its method x path regs).
    call: usize,
    /// Every handler argument is a call expression (`auth("x")`): see
    /// `drop_middleware_regs`.
    mw: bool,
}

#[derive(Default)]
struct Inst {
    base: String,
}

#[derive(Default)]
struct FileGraph {
    rel: String,
    insts: IndexMap<String, Inst>,
    regs: Vec<Reg>,
    mounts: Vec<Mount>,
    /// local name -> (module specifier, imported name or "default" or "*")
    imports: HashMap<String, (String, String)>,
    default_export: Option<String>,
    named: HashMap<String, String>,
    /// Factory function (local name) -> keys of the instances it returns.
    fn_ret: HashMap<String, Vec<String>>,
}

type Node2 = (usize, String);

// ------------------------------------------------------- declarations

/// A statically evaluable string expression (paths, prefixes, consts).
#[derive(Clone, Debug)]
enum Val {
    Lit(String),
    Id(String),
    Member(String, String),
    Tmpl(Vec<Val>),
    Unknown,
}

impl Val {
    /// `ext` enables template substitutions, `a + b` and `ns.NAME`.
    fn from(n: Node, src: &str, ext: bool) -> Val {
        let n = unwrap(n);
        match n.kind() {
            "string" => str_lit(n, src).map(Val::Lit).unwrap_or(Val::Unknown),
            "template_string" => {
                if let Some(s) = str_lit(n, src) {
                    return Val::Lit(s);
                }
                if !ext {
                    return Val::Unknown;
                }
                let mut parts = Vec::new();
                let mut c = n.walk();
                for ch in n.children(&mut c) {
                    match ch.kind() {
                        "`" => {}
                        "template_substitution" => parts.push(
                            ch.named_child(0)
                                .map(|x| Val::from(x, src, ext))
                                .unwrap_or(Val::Unknown),
                        ),
                        _ => parts.push(Val::Lit(txt(ch, src).to_string())),
                    }
                }
                Val::Tmpl(parts)
            }
            "identifier" => Val::Id(txt(n, src).to_string()),
            "member_expression" if ext => {
                match (
                    n.child_by_field_name("object").map(unwrap),
                    n.child_by_field_name("property"),
                ) {
                    (Some(o), Some(p)) if o.kind() == "identifier" => {
                        Val::Member(txt(o, src).to_string(), txt(p, src).to_string())
                    }
                    _ => Val::Unknown,
                }
            }
            "binary_expression" if ext => {
                let op = n.child_by_field_name("operator").map(|o| txt(o, src));
                match (
                    op,
                    n.child_by_field_name("left"),
                    n.child_by_field_name("right"),
                ) {
                    (Some("+"), Some(l), Some(r)) => {
                        Val::Tmpl(vec![Val::from(l, src, ext), Val::from(r, src, ext)])
                    }
                    _ => Val::Unknown,
                }
            }
            _ => Val::Unknown,
        }
    }
}

/// One `createRoute({ method, path })` call.
#[derive(Clone)]
struct RouteDef {
    key: usize,
    line: u32,
    method: Val,
    path: Val,
}

fn route_def_raw(call: Node, src: &str, ext: bool) -> Option<RouteDef> {
    if call.kind() != "call_expression" {
        return None;
    }
    let f = call.child_by_field_name("function")?;
    if txt(f, src) != "createRoute" {
        return None;
    }
    let obj = unwrap(call.child_by_field_name("arguments")?.named_child(0)?);
    Some(RouteDef {
        key: call.start_byte(),
        line: line_of(call),
        method: Val::from(obj_prop(obj, "method", src)?, src, ext),
        path: Val::from(obj_prop(obj, "path", src)?, src, ext),
    })
}

/// A value that can lead to a `createRoute()` definition: a reference
/// (`a`, `a.b`, `a["b"].c`), an inline `createRoute({...})` call, or an
/// object literal grouping them.
#[derive(Clone)]
enum RNode {
    Chain(String, Vec<String>),
    Def(RouteDef),
    Obj(Vec<(String, RNode)>),
}

/// `a`, `a.b`, `a["b"].c` as `(base identifier, property names)`.
fn expr_chain(n: Node, src: &str) -> Option<(String, Vec<String>)> {
    let n = unwrap(n);
    match n.kind() {
        "identifier" => Some((txt(n, src).to_string(), vec![])),
        "member_expression" => {
            let (b, mut p) = expr_chain(n.child_by_field_name("object")?, src)?;
            p.push(txt(n.child_by_field_name("property")?, src).to_string());
            Some((b, p))
        }
        "subscript_expression" => {
            let (b, mut p) = expr_chain(n.child_by_field_name("object")?, src)?;
            p.push(str_lit(n.child_by_field_name("index")?, src)?);
            Some((b, p))
        }
        _ => None,
    }
}

fn route_node(n: Node, src: &str, ext: bool) -> Option<RNode> {
    let n = unwrap(n);
    match n.kind() {
        "call_expression" => route_def_raw(n, src, ext).map(RNode::Def),
        "object" => {
            let mut entries = Vec::new();
            let mut c = n.walk();
            for ch in n.named_children(&mut c) {
                match ch.kind() {
                    "pair" => {
                        let (Some(k), Some(v)) = (
                            ch.child_by_field_name("key"),
                            ch.child_by_field_name("value"),
                        ) else {
                            continue;
                        };
                        if k.kind() == "computed_property_name" {
                            continue;
                        }
                        let key = txt(k, src)
                            .trim_matches(|c| c == '"' || c == '\'')
                            .to_string();
                        if let Some(node) = route_node(v, src, ext) {
                            entries.push((key, node));
                        }
                    }
                    "shorthand_property_identifier" => {
                        let nm = txt(ch, src).to_string();
                        entries.push((nm.clone(), RNode::Chain(nm, vec![])));
                    }
                    _ => {}
                }
            }
            (!entries.is_empty()).then_some(RNode::Obj(entries))
        }
        _ => expr_chain(n, src).map(|(b, p)| RNode::Chain(b, p)),
    }
}

/// Result of resolving a route reference.
enum Found<'r> {
    Def(usize, &'r RouteDef),
    Obj(usize, &'r [(String, RNode)]),
}

/// `(module, Some([(exported, imported)]))`, or `(module, None)` for `export *`.
type ReExport = (String, Option<Vec<(String, String)>>);

/// Cross-file facts about one source file (imports, exports, consts, route
/// definitions). Built in one walk, independent of the framework flavor.
#[derive(Default)]
struct FileDecl {
    rel: String,
    imports: HashMap<String, (String, String)>,
    /// exported name -> local name
    named: HashMap<String, String>,
    default_local: Option<String>,
    /// `(module, Some([(exported, imported)]))` or `(module, None)` for `export *`
    reexports: Vec<ReExport>,
    consts: HashMap<String, Val>,
    route_defs: HashMap<String, RouteDef>,
    /// Object literals (and aliases) that group route definitions.
    route_nodes: HashMap<String, RNode>,
    all_defs: Vec<RouteDef>,
    type_aliases: Vec<String>,
}

fn decl_walk(n: Node, src: &str, ext: bool, d: &mut FileDecl) {
    match n.kind() {
        "import_statement" => decl_import(n, src, d),
        "export_statement" => decl_export(n, src, d),
        "variable_declarator" => {
            if let (Some(name), Some(v)) = (
                n.child_by_field_name("name")
                    .filter(|x| x.kind() == "identifier"),
                n.child_by_field_name("value"),
            ) {
                let nm = txt(name, src).to_string();
                let val = Val::from(v, src, ext);
                if match &val {
                    Val::Unknown => false,
                    Val::Lit(_) => true,
                    _ => ext,
                } {
                    d.consts.insert(nm.clone(), val);
                }
                if ext {
                    if let Some(rd) = route_def_raw(unwrap(v), src, ext) {
                        d.route_defs.insert(nm, rd);
                    } else if let Some(node @ (RNode::Obj(_) | RNode::Chain(..))) =
                        route_node(v, src, ext)
                    {
                        d.route_nodes.insert(nm, node);
                    }
                }
            }
        }
        "call_expression" if ext => {
            if let Some(rd) = route_def_raw(n, src, ext) {
                d.all_defs.push(rd);
            }
        }
        "type_alias_declaration" if ext => {
            if let (Some(name), Some(v)) = (
                n.child_by_field_name("name"),
                n.child_by_field_name("value"),
            ) {
                if txt(v, src).contains("Hono") {
                    d.type_aliases.push(txt(name, src).to_string());
                }
            }
        }
        _ => {}
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        decl_walk(ch, src, ext, d);
    }
}

fn decl_import(n: Node, src: &str, d: &mut FileDecl) {
    let Some(spec) = n
        .child_by_field_name("source")
        .and_then(|s| str_lit(s, src))
    else {
        return;
    };
    let mut c = n.walk();
    for clause in n.children(&mut c).filter(|x| x.kind() == "import_clause") {
        let mut c2 = clause.walk();
        for ch in clause.children(&mut c2) {
            match ch.kind() {
                "identifier" => {
                    d.imports
                        .insert(txt(ch, src).to_string(), (spec.clone(), "default".into()));
                }
                "namespace_import" => {
                    if let Some(id) = ch.named_child(0) {
                        d.imports
                            .insert(txt(id, src).to_string(), (spec.clone(), "*".into()));
                    }
                }
                "named_imports" => {
                    let mut c3 = ch.walk();
                    for sp in ch
                        .named_children(&mut c3)
                        .filter(|x| x.kind() == "import_specifier")
                    {
                        let Some(name) = sp.child_by_field_name("name") else {
                            continue;
                        };
                        let local = sp.child_by_field_name("alias").unwrap_or(name);
                        d.imports.insert(
                            txt(local, src).to_string(),
                            (spec.clone(), txt(name, src).to_string()),
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

fn export_specifiers(n: Node, src: &str) -> Vec<(String, String)> {
    // (exported, local-or-imported)
    let mut out = Vec::new();
    let mut c = n.walk();
    for clause in n.children(&mut c).filter(|x| x.kind() == "export_clause") {
        let mut c2 = clause.walk();
        for sp in clause
            .named_children(&mut c2)
            .filter(|x| x.kind() == "export_specifier")
        {
            let Some(name) = sp.child_by_field_name("name") else {
                continue;
            };
            let exported = sp.child_by_field_name("alias").unwrap_or(name);
            out.push((txt(exported, src).to_string(), txt(name, src).to_string()));
        }
    }
    out
}

fn decl_export(n: Node, src: &str, d: &mut FileDecl) {
    if let Some(spec) = n
        .child_by_field_name("source")
        .and_then(|s| str_lit(s, src))
    {
        let specs = export_specifiers(n, src);
        let mut c = n.walk();
        let has_clause = n.children(&mut c).any(|x| x.kind() == "export_clause");
        if has_clause {
            d.reexports.push((spec, Some(specs)));
        } else {
            d.reexports.push((spec, None));
        }
        return;
    }
    let mut c = n.walk();
    let is_default = n.children(&mut c).any(|x| x.kind() == "default");
    if is_default {
        if let Some(decl) = n.child_by_field_name("declaration") {
            let name = decl
                .child_by_field_name("name")
                .map(|x| txt(x, src).to_string());
            d.default_local = Some(name.unwrap_or_else(|| "<default>".to_string()));
        } else if let Some(v) = n.child_by_field_name("value") {
            let v = unwrap(v);
            if v.kind() == "identifier" {
                d.default_local = Some(txt(v, src).to_string());
            } else if is_function_node(v) {
                d.default_local = Some("<default>".to_string());
            }
        }
        return;
    }
    if let Some(decl) = n.child_by_field_name("declaration") {
        match decl.kind() {
            "lexical_declaration" | "variable_declaration" => {
                let mut c = decl.walk();
                for x in decl
                    .named_children(&mut c)
                    .filter(|x| x.kind() == "variable_declarator")
                {
                    if let Some(name) = x
                        .child_by_field_name("name")
                        .filter(|x| x.kind() == "identifier")
                    {
                        let nm = txt(name, src).to_string();
                        d.named.insert(nm.clone(), nm);
                    }
                }
            }
            _ => {
                if let Some(name) = decl.child_by_field_name("name") {
                    let nm = txt(name, src).to_string();
                    d.named.insert(nm.clone(), nm);
                }
            }
        }
    }
    for (exported, local) in export_specifiers(n, src) {
        if exported == "default" {
            d.default_local = Some(local);
        } else {
            d.named.insert(exported, local);
        }
    }
}

/// Top-level function-like declarations: `(local name, function node)`.
fn top_funcs<'t>(n: Node<'t>, src: &str, out: &mut Vec<(String, Node<'t>)>) {
    match n.kind() {
        "function_declaration" | "generator_function_declaration" => {
            if let Some(name) = n.child_by_field_name("name") {
                out.push((txt(name, src).to_string(), n));
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut c = n.walk();
            for x in n
                .named_children(&mut c)
                .filter(|x| x.kind() == "variable_declarator")
            {
                if let (Some(name), Some(v)) = (
                    x.child_by_field_name("name")
                        .filter(|x| x.kind() == "identifier"),
                    x.child_by_field_name("value").map(unwrap),
                ) {
                    if is_function_node(v) {
                        out.push((txt(name, src).to_string(), v));
                    }
                }
            }
        }
        "export_statement" => {
            let mut c = n.walk();
            for ch in n.named_children(&mut c) {
                if is_function_node(ch) {
                    out.push(("<default>".to_string(), ch));
                } else {
                    top_funcs(ch, src, out);
                }
            }
        }
        _ => {}
    }
}

fn walk_fn_body<'t>(
    n: Node<'t>,
    src: &str,
    returns: &mut Vec<Node<'t>>,
    locals: &mut HashMap<String, Node<'t>>,
) {
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        if is_function_node(ch)
            || matches!(
                ch.kind(),
                "function_declaration" | "class_declaration" | "method_definition"
            )
        {
            continue;
        }
        match ch.kind() {
            "return_statement" => {
                if let Some(e) = ch.named_child(0) {
                    returns.push(e);
                }
            }
            "variable_declarator" => {
                if let (Some(name), Some(v)) = (
                    ch.child_by_field_name("name")
                        .filter(|x| x.kind() == "identifier"),
                    ch.child_by_field_name("value"),
                ) {
                    locals.insert(txt(name, src).to_string(), v);
                }
            }
            _ => {}
        }
        walk_fn_body(ch, src, returns, locals);
    }
}

// ----------------------------------------------------------- resolver

const CHAIN_METHODS: [&str; 24] = [
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "options",
    "head",
    "all",
    "on",
    "use",
    "route",
    "basePath",
    "openapi",
    "doc",
    "doc31",
    "mount",
    "onError",
    "notFound",
    "param",
    "set",
    "enable",
    "disable",
    "engine",
    "getOpenAPIDocument",
];

/// Cross-file lookups over all parsed files: modules, exports (with
/// re-exports), string consts and factory functions.
struct Resolver<'a> {
    decls: &'a [FileDecl],
    mods: &'a Modules,
    factories: HashSet<(usize, String)>,
    /// Hono only: template substitutions, imported consts, factories, route defs.
    ext: bool,
    hono_types: Vec<String>,
}

impl<'a> Resolver<'a> {
    fn module(&self, fi: usize, spec: &str) -> Option<usize> {
        self.mods.resolve(fi, spec)
    }

    /// Follow `name` exported by file `fi` (through re-exports) to the file
    /// and local name that declare it.
    fn export_in(&self, fi: usize, name: &str, depth: u8) -> Option<(usize, String)> {
        if depth > 6 {
            return None;
        }
        let d = &self.decls[fi];
        if name == "default" {
            if let Some(l) = &d.default_local {
                return Some((fi, l.clone()));
            }
        } else if let Some(l) = d.named.get(name) {
            return Some((fi, l.clone()));
        }
        for (spec, items) in &d.reexports {
            let Some(fk) = self.module(fi, spec) else {
                continue;
            };
            match items {
                Some(items) => {
                    if let Some((_, imported)) = items.iter().find(|(e, _)| e == name) {
                        if let Some(r) = self.export_in(fk, imported, depth + 1) {
                            return Some(r);
                        }
                    }
                }
                None if name != "default" => {
                    if let Some(r) = self.export_in(fk, name, depth + 1) {
                        return Some(r);
                    }
                }
                None => {}
            }
        }
        None
    }

    fn resolve_import(&self, fi: usize, spec: &str, imp: &str) -> Option<(usize, String)> {
        let fj = self.module(fi, spec)?;
        self.export_in(fj, imp, 0)
    }

    /// Look `name` up in file `fi` as a route definition or a route object,
    /// following imports and aliases.
    fn lookup_route(&self, fi: usize, name: &str, depth: u8) -> Option<Found<'a>> {
        if depth > 8 {
            return None;
        }
        let d = &self.decls[fi];
        if let Some(rd) = d.route_defs.get(name) {
            return Some(Found::Def(fi, rd));
        }
        match d.route_nodes.get(name) {
            Some(RNode::Obj(o)) => return Some(Found::Obj(fi, o)),
            Some(RNode::Chain(b, p)) => return self.follow_route(fi, b, p, depth + 1),
            _ => {}
        }
        let (spec, imp) = d.imports.get(name)?;
        if imp == "*" {
            return None;
        }
        let (fj, local) = self.resolve_import(fi, spec, imp)?;
        self.lookup_route(fj, &local, depth + 1)
    }

    /// Resolve `base.p0.p1...` as seen from file `fi`.
    fn follow_route(
        &self,
        fi: usize,
        base: &str,
        props: &[String],
        depth: u8,
    ) -> Option<Found<'a>> {
        if depth > 8 {
            return None;
        }
        if let Some((spec, imp)) = self.decls[fi].imports.get(base) {
            if imp == "*" {
                let (first, rest) = props.split_first()?;
                let fj = self.module(fi, spec)?;
                let (fk, local) = self.export_in(fj, first, 0)?;
                let f = self.lookup_route(fk, &local, depth + 1)?;
                return self.descend_route(f, rest, depth + 1);
            }
        }
        let f = self.lookup_route(fi, base, depth + 1)?;
        self.descend_route(f, props, depth + 1)
    }

    fn descend_route(&self, f: Found<'a>, props: &[String], depth: u8) -> Option<Found<'a>> {
        let mut cur = f;
        for p in props {
            let Found::Obj(fi, o) = cur else { return None };
            cur = match &o.iter().find(|(k, _)| k == p)?.1 {
                RNode::Def(d) => Found::Def(fi, d),
                RNode::Obj(o2) => Found::Obj(fi, o2),
                RNode::Chain(b, pp) => self.follow_route(fi, b, pp, depth + 1)?,
            };
        }
        Some(cur)
    }

    fn is_factory_callee(&self, fi: usize, name: &str) -> bool {
        if self.factories.contains(&(fi, name.to_string())) {
            return true;
        }
        self.decls[fi]
            .imports
            .get(name)
            .and_then(|(s, i)| self.resolve_import(fi, s, i))
            .is_some_and(|k| self.factories.contains(&k))
    }

    fn eval(&self, v: &Val, fi: usize) -> Option<String> {
        self.eval_d(v, fi, 0)
    }

    fn eval_d(&self, v: &Val, fi: usize, d: u8) -> Option<String> {
        if d > 8 {
            return None;
        }
        match v {
            Val::Lit(s) => Some(s.clone()),
            Val::Id(name) => {
                if let Some(c) = self.decls[fi].consts.get(name) {
                    return self.eval_d(c, fi, d + 1);
                }
                if !self.ext {
                    return None;
                }
                let (spec, imp) = self.decls[fi].imports.get(name)?;
                let (fj, local) = self.resolve_import(fi, spec, imp)?;
                let c = self.decls[fj].consts.get(&local)?;
                self.eval_d(c, fj, d + 1)
            }
            Val::Member(ns, prop) => {
                let (spec, imp) = self.decls[fi].imports.get(ns)?;
                if imp != "*" {
                    return None;
                }
                let fj = self.module(fi, spec)?;
                let (fk, local) = self.export_in(fj, prop, 0)?;
                let c = self.decls[fk].consts.get(&local)?;
                self.eval_d(c, fk, d + 1)
            }
            Val::Tmpl(parts) => {
                let mut s = String::new();
                for p in parts {
                    s.push_str(&self.eval_d(p, fi, d + 1)?);
                }
                Some(s)
            }
            Val::Unknown => None,
        }
    }

    /// Whether `e` evaluates to a Hono instance (`new Hono()`, a factory
    /// call, or a local variable holding one), looking through chains.
    fn is_hono_expr<'t>(
        &self,
        e: Node<'t>,
        fi: usize,
        src: &str,
        locals: &HashMap<String, Node<'t>>,
        depth: u8,
    ) -> bool {
        if depth > 6 {
            return false;
        }
        let mut n = unwrap(e);
        loop {
            match n.kind() {
                "new_expression" => {
                    return n
                        .child_by_field_name("constructor")
                        .is_some_and(|c| txt(c, src).ends_with("Hono"));
                }
                "call_expression" => {
                    let Some(f) = n.child_by_field_name("function").map(unwrap) else {
                        return false;
                    };
                    match f.kind() {
                        "member_expression" => {
                            let prop = f
                                .child_by_field_name("property")
                                .map(|p| txt(p, src))
                                .unwrap_or("");
                            let Some(o) = f.child_by_field_name("object") else {
                                return false;
                            };
                            if !CHAIN_METHODS.contains(&prop) {
                                return false;
                            }
                            n = unwrap(o);
                        }
                        "identifier" => {
                            if self.is_factory_callee(fi, txt(f, src)) {
                                return true;
                            }
                            // `wrap(new Hono())`: look through wrapper calls.
                            let first = n
                                .child_by_field_name("arguments")
                                .and_then(|a| a.named_child(0));
                            return first
                                .is_some_and(|a| self.is_hono_expr(a, fi, src, locals, depth + 1));
                        }
                        _ => return false,
                    }
                }
                "identifier" => {
                    return match locals.get(txt(n, src)) {
                        Some(v) => self.is_hono_expr(*v, fi, src, locals, depth + 1),
                        None => false,
                    };
                }
                _ => return false,
            }
        }
    }

    fn func_returns_hono(&self, f: Node, fi: usize, src: &str) -> bool {
        if f.child_by_field_name("return_type")
            .is_some_and(|t| txt(t, src).contains("Hono"))
        {
            return true;
        }
        let Some(body) = f.child_by_field_name("body") else {
            return false;
        };
        let mut returns = Vec::new();
        let mut locals = HashMap::new();
        if body.kind() == "statement_block" {
            walk_fn_body(body, src, &mut returns, &mut locals);
        } else {
            returns.push(body);
        }
        returns
            .into_iter()
            .any(|r| self.is_hono_expr(r, fi, src, &locals, 0))
    }
}

/// Calls to factory functions below `n` (keyed by byte range).
fn factory_calls(n: Node, src: &str, res: &Resolver, fi: usize, out: &mut HashSet<(usize, usize)>) {
    if n.kind() == "call_expression" {
        if let Some(f) = n.child_by_field_name("function").map(unwrap) {
            if f.kind() == "identifier" && res.is_factory_callee(fi, txt(f, src)) {
                out.insert((n.start_byte(), n.end_byte()));
            }
        }
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        factory_calls(ch, src, res, fi, out);
    }
}

// ------------------------------------------------------------- builder

struct Builder<'a> {
    src: &'a str,
    flavor: Flavor,
    fi: usize,
    res: &'a Resolver<'a>,
    g: FileGraph,
    fcalls: HashSet<(usize, usize)>,
    creations: HashMap<usize, String>,
    derived: HashMap<usize, String>,
    /// `const routes = [a, b]`: names of the identifiers in the array.
    arrays: HashMap<String, Vec<String>>,
    /// Loop variables bound to the elements of a known array.
    loop_vars: HashMap<String, Vec<String>>,
    /// createRoute() definitions consumed by `.openapi()`: (file, key).
    used: HashSet<(usize, usize)>,
    /// Factory calls already linked to the instances the factory returns.
    linked: HashSet<usize>,
    /// Lines of `.openapi()` calls whose route reference was not resolvable.
    unresolved: Vec<u32>,
    warnings: Vec<String>,
}

impl<'a> Builder<'a> {
    fn t(&self, n: Node<'a>) -> &'a str {
        txt(n, self.src)
    }

    fn warn(&mut self, line: u32, msg: &str) {
        self.warnings
            .push(format!("{}:{}: {}", self.g.rel, line, msg));
    }

    fn str_of(&self, n: Node<'a>) -> Option<String> {
        let v = Val::from(n, self.src, self.res.ext);
        self.res.eval(&v, self.fi)
    }

    fn args(&self, call: Node<'a>) -> Vec<Node<'a>> {
        let Some(a) = call.child_by_field_name("arguments") else {
            return vec![];
        };
        let mut c = a.walk();
        let v: Vec<Node<'a>> = a
            .named_children(&mut c)
            .filter(|x| x.kind() != "comment")
            .collect();
        v
    }

    fn is_creation(&self, n: Node<'a>) -> bool {
        match (self.flavor, n.kind()) {
            (Flavor::Hono, "new_expression") => n
                .child_by_field_name("constructor")
                .map(|c| self.t(c).ends_with("Hono"))
                .unwrap_or(false),
            (Flavor::Hono, "call_expression") => {
                self.fcalls.contains(&(n.start_byte(), n.end_byte()))
            }
            (Flavor::Express, "call_expression") => n
                .child_by_field_name("function")
                .map(|f| match f.kind() {
                    "identifier" => matches!(self.t(f), "express" | "Router"),
                    "member_expression" => {
                        f.child_by_field_name("property").map(|p| self.t(p)) == Some("Router")
                    }
                    _ => false,
                })
                .unwrap_or(false),
            _ => false,
        }
    }

    fn ensure_inst(&mut self, key: &str) {
        if !self.g.insts.contains_key(key) {
            self.g.insts.insert(key.to_string(), Inst::default());
        }
    }

    fn creation_key(&mut self, n: Node<'a>) -> String {
        if let Some(k) = self.creations.get(&n.start_byte()) {
            return k.clone();
        }
        let k = format!("@{}", n.start_byte());
        self.creations.insert(n.start_byte(), k.clone());
        self.ensure_inst(&k);
        k
    }

    /// Follow `a.b().c()` down to the receiver expression of the chain.
    fn chain_base(&self, mut n: Node<'a>) -> Option<Node<'a>> {
        loop {
            n = unwrap(n);
            if self.is_creation(n) {
                return Some(n);
            }
            if n.kind() == "call_expression" {
                let f = unwrap(n.child_by_field_name("function")?);
                if f.kind() == "member_expression" {
                    n = f.child_by_field_name("object")?;
                    continue;
                }
                // `wrap(new Hono())`: look through a wrapper call.
                if self.flavor == Flavor::Hono && f.kind() == "identifier" {
                    if let Some(inner) = self.args(n).first().and_then(|a| self.chain_base(*a)) {
                        if self.is_creation(inner) {
                            return Some(inner);
                        }
                    }
                }
            }
            return Some(n);
        }
    }

    /// Instance key an expression evaluates to (creation chains and identifiers).
    fn expr_instance_name(&mut self, e: Node<'a>) -> Option<String> {
        let base = self.chain_base(e)?;
        if self.is_creation(base) {
            Some(self.creation_key(base))
        } else if base.kind() == "identifier" {
            Some(self.t(base).to_string())
        } else {
            None
        }
    }

    fn require_of(&self, v: Node<'a>) -> Option<(String, Option<String>)> {
        let u = unwrap(v);
        let (call, member) = if u.kind() == "member_expression" {
            (
                unwrap(u.child_by_field_name("object")?),
                u.child_by_field_name("property")
                    .map(|p| self.t(p).to_string()),
            )
        } else {
            (u, None)
        };
        if call.kind() != "call_expression" {
            return None;
        }
        let f = call.child_by_field_name("function")?;
        if f.kind() != "identifier" || self.t(f) != "require" {
            return None;
        }
        let spec = self.str_of(call.child_by_field_name("arguments")?.named_child(0)?)?;
        Some((spec, member))
    }

    /// Resolve a route reference (`createRoute(...)`, a variable, a named
    /// import, `ns.name`, or a property path into a const object literal)
    /// to its definition and defining file.
    fn route_def_ref(&self, n: Node<'a>) -> Option<(usize, RouteDef)> {
        let u = unwrap(n);
        if u.kind() == "call_expression" {
            return route_def_raw(u, self.src, self.res.ext).map(|d| (self.fi, d));
        }
        // `name`, `ns.name`, `obj.prop`, `obj["prop"]`, `a.b.c`, local or imported.
        let (base, props) = expr_chain(u, self.src)?;
        match self.res.follow_route(self.fi, &base, &props, 0)? {
            Found::Def(f, d) => Some((f, d.clone())),
            Found::Obj(..) => None,
        }
    }

    // ------------------------------------------------ pass 1: declarations

    fn collect(&mut self, n: Node<'a>) {
        match n.kind() {
            "import_statement" => self.import(n),
            "export_statement" => self.export(n),
            "variable_declarator" => self.declarator(n),
            "assignment_expression" => self.assignment(n),
            "required_parameter" | "optional_parameter" => self.param(n),
            _ => {}
        }
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            self.collect(ch);
        }
    }

    fn import(&mut self, n: Node<'a>) {
        let Some(spec) = n
            .child_by_field_name("source")
            .and_then(|s| str_lit(s, self.src))
        else {
            return;
        };
        let mut c = n.walk();
        for clause in n.children(&mut c).filter(|x| x.kind() == "import_clause") {
            let mut c2 = clause.walk();
            for ch in clause.children(&mut c2) {
                match ch.kind() {
                    "identifier" => {
                        self.g
                            .imports
                            .insert(self.t(ch).to_string(), (spec.clone(), "default".into()));
                    }
                    "namespace_import" => {
                        if let Some(id) = ch.named_child(0) {
                            self.g
                                .imports
                                .insert(self.t(id).to_string(), (spec.clone(), "*".into()));
                        }
                    }
                    "named_imports" => {
                        let mut c3 = ch.walk();
                        for sp in ch
                            .named_children(&mut c3)
                            .filter(|x| x.kind() == "import_specifier")
                        {
                            let Some(name) = sp.child_by_field_name("name") else {
                                continue;
                            };
                            let local = sp.child_by_field_name("alias").unwrap_or(name);
                            self.g.imports.insert(
                                self.t(local).to_string(),
                                (spec.clone(), self.t(name).to_string()),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn export(&mut self, n: Node<'a>) {
        if let Some(v) = n.child_by_field_name("value") {
            if let Some(k) = self.expr_instance_name(v) {
                self.g.default_export = Some(k);
            }
        }
        let has_source = n.child_by_field_name("source").is_some();
        if let Some(d) = n.child_by_field_name("declaration") {
            if matches!(d.kind(), "lexical_declaration" | "variable_declaration") {
                let mut c = d.walk();
                for decl in d
                    .named_children(&mut c)
                    .filter(|x| x.kind() == "variable_declarator")
                {
                    if let Some(name) = decl
                        .child_by_field_name("name")
                        .filter(|x| x.kind() == "identifier")
                    {
                        let nm = self.t(name).to_string();
                        self.g.named.insert(nm.clone(), nm);
                    }
                }
            }
        }
        if has_source {
            return;
        }
        let mut c = n.walk();
        for clause in n.children(&mut c).filter(|x| x.kind() == "export_clause") {
            let mut c2 = clause.walk();
            for sp in clause
                .named_children(&mut c2)
                .filter(|x| x.kind() == "export_specifier")
            {
                let Some(name) = sp.child_by_field_name("name") else {
                    continue;
                };
                let exported = sp.child_by_field_name("alias").unwrap_or(name);
                let (local, exp) = (self.t(name).to_string(), self.t(exported).to_string());
                if exp == "default" {
                    self.g.default_export = Some(local);
                } else {
                    self.g.named.insert(exp, local);
                }
            }
        }
    }

    fn declarator(&mut self, n: Node<'a>) {
        let (Some(name), value) = (
            n.child_by_field_name("name"),
            n.child_by_field_name("value"),
        ) else {
            return;
        };
        let Some(v) = value else { return };
        if name.kind() == "object_pattern" {
            if let Some((spec, _)) = self.require_of(v) {
                let mut c = name.walk();
                for ch in name.named_children(&mut c) {
                    match ch.kind() {
                        "shorthand_property_identifier_pattern" => {
                            let nm = self.t(ch).to_string();
                            self.g.imports.insert(nm.clone(), (spec.clone(), nm));
                        }
                        "pair_pattern" => {
                            if let (Some(k), Some(val)) = (
                                ch.child_by_field_name("key"),
                                ch.child_by_field_name("value"),
                            ) {
                                self.g.imports.insert(
                                    self.t(val).to_string(),
                                    (spec.clone(), self.t(k).to_string()),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            return;
        }
        if name.kind() != "identifier" {
            return;
        }
        let nm = self.t(name).to_string();
        if let Some((spec, imported)) = self.require_of(v) {
            self.g.imports.insert(
                nm.clone(),
                (spec, imported.unwrap_or_else(|| "default".into())),
            );
        }
        let u = unwrap(v);
        if u.kind() == "array" {
            let mut c = u.walk();
            let names: Vec<String> = u
                .named_children(&mut c)
                .map(unwrap)
                .filter(|e| e.kind() == "identifier")
                .map(|e| self.t(e).to_string())
                .collect();
            self.arrays.insert(nm.clone(), names);
            return;
        }
        if let Some(base) = self.chain_base(v) {
            if self.is_creation(base) {
                self.creations.insert(base.start_byte(), nm.clone());
                self.ensure_inst(&nm);
                return;
            }
        }
        // `const v1 = app.basePath('/v1')`
        if u.kind() == "call_expression" {
            if let Some(f) = u
                .child_by_field_name("function")
                .map(unwrap)
                .filter(|f| f.kind() == "member_expression")
            {
                let prop = f.child_by_field_name("property").map(|p| self.t(p));
                let obj = f.child_by_field_name("object").map(unwrap);
                if let (Some("basePath"), Some(obj)) = (prop, obj) {
                    if obj.kind() == "identifier" && self.g.insts.contains_key(self.t(obj)) {
                        let bp = self
                            .args(u)
                            .first()
                            .and_then(|a| self.str_of(*a))
                            .unwrap_or_default();
                        let parent = self.t(obj).to_string();
                        self.g.insts.insert(nm.clone(), Inst { base: bp });
                        self.derived.insert(u.start_byte(), nm.clone());
                        self.g.mounts.push(Mount {
                            parent,
                            prefix: String::new(),
                            child: ChildRef::Key(nm),
                            line: line_of(n),
                        });
                    }
                }
            }
        }
    }

    fn assignment(&mut self, n: Node<'a>) {
        let (Some(left), Some(right)) = (
            n.child_by_field_name("left"),
            n.child_by_field_name("right"),
        ) else {
            return;
        };
        let lt = self.t(left).to_string();
        if lt == "module.exports" {
            let r = unwrap(right);
            if r.kind() == "object" {
                let mut c = r.walk();
                for ch in r.named_children(&mut c) {
                    match ch.kind() {
                        "shorthand_property_identifier" => {
                            let nm = self.t(ch).to_string();
                            self.g.named.insert(nm.clone(), nm);
                        }
                        "pair" => {
                            if let (Some(k), Some(v)) = (
                                ch.child_by_field_name("key"),
                                ch.child_by_field_name("value"),
                            ) {
                                if v.kind() == "identifier" {
                                    self.g
                                        .named
                                        .insert(self.t(k).to_string(), self.t(v).to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            } else if let Some(k) = self.expr_instance_name(right) {
                self.g.default_export = Some(k);
            }
        } else if let Some(name) = lt
            .strip_prefix("module.exports.")
            .or_else(|| lt.strip_prefix("exports."))
        {
            if let Some(k) = self.expr_instance_name(right) {
                self.g.named.insert(name.to_string(), k);
            }
        }
    }

    fn param(&mut self, n: Node<'a>) {
        let (Some(pat), Some(ty)) = (
            n.child_by_field_name("pattern"),
            n.child_by_field_name("type"),
        ) else {
            return;
        };
        if pat.kind() != "identifier" {
            return;
        }
        let ty = self.t(ty);
        let hit = match self.flavor {
            Flavor::Hono => {
                ty.contains("Hono") || self.res.hono_types.iter().any(|a| ty.contains(a.as_str()))
            }
            Flavor::Express => {
                ty.contains("Express") || ty.contains("Router") || ty.contains("Application")
            }
        };
        if hit {
            let nm = self.t(pat).to_string();
            self.ensure_inst(&nm);
        }
    }

    /// Record, for every factory function declared in this file, the
    /// instances its `return` expressions evaluate to.
    fn collect_fn_returns(&mut self, root: Node<'a>) {
        let mut funcs = Vec::new();
        let mut c = root.walk();
        for top in root.named_children(&mut c) {
            top_funcs(top, self.src, &mut funcs);
        }
        for (name, f) in funcs {
            if !self.res.factories.contains(&(self.fi, name.clone())) {
                continue;
            }
            let Some(body) = f.child_by_field_name("body") else {
                continue;
            };
            let mut returns = Vec::new();
            let mut locals = HashMap::new();
            if body.kind() == "statement_block" {
                walk_fn_body(body, self.src, &mut returns, &mut locals);
            } else {
                returns.push(body);
            }
            let mut keys: Vec<String> = Vec::new();
            for r in returns {
                if let Some(k) = self.expr_instance_name(r) {
                    let own = k.starts_with('@') || locals.contains_key(&k);
                    if own && self.g.insts.contains_key(&k) && !keys.contains(&k) {
                        keys.push(k);
                    }
                }
            }
            if !keys.is_empty() {
                self.g.fn_ret.insert(name, keys);
            }
        }
    }

    /// `createX()` of a known factory: the call site becomes an instance that
    /// stands for whatever the factory returns (mounted with an empty prefix).
    fn link_factory_call(&mut self, n: Node<'a>) {
        let Some(f) = n.child_by_field_name("function").map(unwrap) else {
            return;
        };
        if f.kind() != "identifier" {
            return;
        }
        let key = self.creation_key(n);
        if self.linked.insert(n.start_byte()) {
            self.g.mounts.push(Mount {
                parent: key,
                prefix: String::new(),
                child: ChildRef::Factory(self.t(f).to_string()),
                line: line_of(n),
            });
        }
    }

    // --------------------------------------------------- pass 2: calls

    fn resolve_receiver(&mut self, n: Node<'a>) -> Option<Recv> {
        let n = unwrap(n);
        if self.is_creation(n) {
            return Some(Recv {
                key: self.creation_key(n),
                sub: None,
            });
        }
        match n.kind() {
            "identifier" => {
                let t = self.t(n);
                self.g.insts.contains_key(t).then(|| Recv {
                    key: t.to_string(),
                    sub: None,
                })
            }
            "call_expression" => {
                let f = unwrap(n.child_by_field_name("function")?);
                if f.kind() != "member_expression" {
                    return None;
                }
                let obj = f.child_by_field_name("object")?;
                let prop = self.t(f.child_by_field_name("property")?);
                if !CHAIN_METHODS.contains(&prop) {
                    return None;
                }
                let r = self.resolve_receiver(obj)?;
                let args = self.args(n);
                match prop {
                    "basePath" => {
                        let bp = self.str_of(*args.first()?)?;
                        if self.is_creation(unwrap(obj)) {
                            if let Some(i) = self.g.insts.get_mut(&r.key) {
                                i.base = bp;
                            }
                            Some(r)
                        } else {
                            let start = n.start_byte();
                            let key = if let Some(k) = self.derived.get(&start) {
                                k.clone()
                            } else {
                                let k = format!("@bp{}", start);
                                self.derived.insert(start, k.clone());
                                self.g.insts.insert(k.clone(), Inst { base: bp });
                                self.g.mounts.push(Mount {
                                    parent: r.key.clone(),
                                    prefix: String::new(),
                                    child: ChildRef::Key(k.clone()),
                                    line: line_of(n),
                                });
                                k
                            };
                            Some(Recv { key, sub: None })
                        }
                    }
                    "route"
                        if args.len() == 1 && r.sub.is_none() && self.flavor == Flavor::Express =>
                    {
                        let s = self.str_of(args[0])?;
                        Some(Recv {
                            key: r.key,
                            sub: Some(s),
                        })
                    }
                    _ => Some(r),
                }
            }
            _ => None,
        }
    }

    fn visit(&mut self, n: Node<'a>) {
        let mut bound: Option<String> = None;
        match n.kind() {
            "call_expression" => {
                if self.flavor == Flavor::Hono
                    && self.fcalls.contains(&(n.start_byte(), n.end_byte()))
                {
                    self.link_factory_call(n);
                }
                self.call(n);
                bound = self.bind_for_each(n);
            }
            "for_in_statement" => bound = self.bind_for_of(n),
            _ => {}
        }
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            self.visit(ch);
        }
        if let Some(b) = bound {
            self.loop_vars.remove(&b);
        }
    }

    fn array_names(&self, n: Node<'a>) -> Option<Vec<String>> {
        let u = unwrap(n);
        match u.kind() {
            "identifier" => self.arrays.get(self.t(u)).cloned(),
            "array" => {
                let mut c = u.walk();
                Some(
                    u.named_children(&mut c)
                        .map(unwrap)
                        .filter(|e| e.kind() == "identifier")
                        .map(|e| self.t(e).to_string())
                        .collect(),
                )
            }
            _ => None,
        }
    }

    /// `routes.forEach((route) => ...)` over a known array: bind `route`.
    fn bind_for_each(&mut self, n: Node<'a>) -> Option<String> {
        let f = unwrap(n.child_by_field_name("function")?);
        if f.kind() != "member_expression"
            || self.t(f.child_by_field_name("property")?) != "forEach"
        {
            return None;
        }
        let names = self.array_names(f.child_by_field_name("object")?)?;
        let cb = unwrap(*self.args(n).first()?);
        if !is_function_node(cb) {
            return None;
        }
        let p = if let Some(p) = cb.child_by_field_name("parameter") {
            p
        } else {
            let first = cb.child_by_field_name("parameters")?.named_child(0)?;
            match first.kind() {
                "required_parameter" | "optional_parameter" => {
                    first.child_by_field_name("pattern")?
                }
                _ => first,
            }
        };
        if p.kind() != "identifier" {
            return None;
        }
        let nm = self.t(p).to_string();
        self.loop_vars.insert(nm.clone(), names);
        Some(nm)
    }

    /// `for (const route of routes) ...` over a known array.
    fn bind_for_of(&mut self, n: Node<'a>) -> Option<String> {
        if self.t(n.child_by_field_name("operator")?) != "of" {
            return None;
        }
        let left = n.child_by_field_name("left")?;
        if left.kind() != "identifier" {
            return None;
        }
        let names = self.array_names(n.child_by_field_name("right")?)?;
        let nm = self.t(left).to_string();
        self.loop_vars.insert(nm.clone(), names);
        Some(nm)
    }

    fn path_list(&mut self, n: Node<'a>) -> Option<Vec<String>> {
        let u = unwrap(n);
        if u.kind() == "array" {
            let mut c = u.walk();
            let elems: Vec<Node<'a>> = u
                .named_children(&mut c)
                .filter(|e| e.kind() != "comment")
                .collect();
            let v: Vec<String> = elems.iter().filter_map(|e| self.str_of(*e)).collect();
            if v.len() < elems.len() {
                self.warn(
                    line_of(u),
                    "some paths in the array are not statically known; those were skipped",
                );
            }
            return (!v.is_empty()).then_some(v);
        }
        self.str_of(u).map(|s| vec![s])
    }

    fn child_ref(&mut self, a: Node<'a>) -> Option<ChildRef> {
        let u = unwrap(a);
        match u.kind() {
            "identifier" => Some(ChildRef::Name(self.t(u).to_string())),
            "call_expression" | "new_expression" => {
                if let Some((spec, imp)) = self.require_of(u) {
                    return Some(ChildRef::Require(spec, imp));
                }
                self.resolve_receiver(u).map(|r| ChildRef::Key(r.key))
            }
            "member_expression" => {
                if let Some((spec, _)) = self.require_of(u) {
                    let imp = u
                        .child_by_field_name("property")
                        .map(|p| self.t(p).to_string());
                    return Some(ChildRef::Require(spec, imp));
                }
                let obj = unwrap(u.child_by_field_name("object")?);
                let prop = self.t(u.child_by_field_name("property")?).to_string();
                (obj.kind() == "identifier" && self.flavor == Flavor::Hono)
                    .then(|| ChildRef::NsMember(self.t(obj).to_string(), prop))
            }
            _ => None,
        }
    }

    fn child_refs(&mut self, a: Node<'a>) -> Vec<ChildRef> {
        let u = unwrap(a);
        if u.kind() == "identifier" {
            if let Some(names) = self.loop_vars.get(self.t(u)) {
                return names.iter().cloned().map(ChildRef::Name).collect();
            }
        }
        self.child_ref(a).into_iter().collect()
    }

    fn call(&mut self, n: Node<'a>) {
        let Some(f) = n.child_by_field_name("function").map(unwrap) else {
            return;
        };
        if f.kind() != "member_expression" {
            return;
        }
        let (Some(obj), Some(prop)) = (
            f.child_by_field_name("object"),
            f.child_by_field_name("property"),
        ) else {
            return;
        };
        let m = self.t(prop);
        let line = line_of(prop);
        let args = self.args(n);
        match m {
            "basePath" => {
                let _ = self.resolve_receiver(n);
            }
            "get" | "post" | "put" | "patch" | "delete" | "options" | "head" | "all" | "on" => {
                self.register(obj, m, line, &args)
            }
            "openapi" => {
                let Some(recv) = self.resolve_receiver(obj) else {
                    return;
                };
                let Some(first) = args.first() else { return };
                let Some((fj, def)) = self.route_def_ref(*first) else {
                    if self.flavor == Flavor::Hono {
                        self.unresolved.push(line);
                    }
                    return;
                };
                self.used.insert((fj, def.key));
                let (method, path) = (self.res.eval(&def.method, fj), self.res.eval(&def.path, fj));
                let (Some(method), Some(path)) = (method, path) else {
                    self.warn(
                        line,
                        "createRoute() method or path is not statically known; skipped",
                    );
                    return;
                };
                let method = method.to_ascii_uppercase();
                if !HTTP_METHODS.contains(&method.as_str()) {
                    return;
                }
                let h = args.get(1);
                let handler = h
                    .filter(|a| matches!(a.kind(), "identifier" | "member_expression"))
                    .map(|a| self.t(*a).to_string());
                let mut query = Vec::new();
                if let Some(h) =
                    h.filter(|a| !matches!(a.kind(), "identifier" | "member_expression"))
                {
                    collect_query(*h, self.src, &mut query);
                    dedup(&mut query);
                }
                self.g.regs.push(Reg {
                    inst: recv.key,
                    method,
                    path,
                    line,
                    handler,
                    query,
                    call: n.start_byte(),
                    mw: false,
                });
            }
            "doc" | "doc31" if self.flavor == Flavor::Hono => {
                let Some(recv) = self.resolve_receiver(obj) else {
                    return;
                };
                let Some(path) = args.first().and_then(|a| self.str_of(*a)) else {
                    self.warn(line, "route path is not statically known; skipped");
                    return;
                };
                self.g.regs.push(Reg {
                    inst: recv.key,
                    method: "GET".to_string(),
                    path,
                    line,
                    handler: None,
                    query: vec![],
                    call: n.start_byte(),
                    mw: false,
                });
            }
            "route" if args.len() >= 2 => {
                let Some(recv) = self.resolve_receiver(obj) else {
                    return;
                };
                let Some(prefix) = self.str_of(args[0]) else {
                    self.warn(
                        line,
                        "mount prefix is not statically known; sub-routes skipped",
                    );
                    return;
                };
                for child in self.child_refs(args[1]) {
                    self.g.mounts.push(Mount {
                        parent: recv.key.clone(),
                        prefix: prefix.clone(),
                        child,
                        line,
                    });
                }
            }
            "use" if self.flavor == Flavor::Express => {
                let Some(recv) = self.resolve_receiver(obj) else {
                    return;
                };
                if recv.sub.is_some() || args.is_empty() {
                    return;
                }
                let (prefix, rest) = match self.path_list(args[0]) {
                    Some(p) => (p[0].clone(), &args[1..]),
                    None => (String::new(), &args[..]),
                };
                for a in rest {
                    if let Some(child) = self.child_ref(*a) {
                        self.g.mounts.push(Mount {
                            parent: recv.key.clone(),
                            prefix: prefix.clone(),
                            child,
                            line,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    fn register(&mut self, obj: Node<'a>, m: &str, line: u32, args: &[Node<'a>]) {
        let Some(recv) = self.resolve_receiver(obj) else {
            return;
        };
        let (methods, paths, handlers): (Vec<String>, Vec<String>, &[Node<'a>]) = if m == "on" {
            if args.len() < 2 {
                return;
            }
            let ms = self.path_list(args[0]).unwrap_or_default();
            let Some(ps) = self.path_list(args[1]) else {
                self.warn(line, "route path is not statically known; skipped");
                return;
            };
            (
                ms.iter().map(|s| s.to_ascii_uppercase()).collect(),
                ps,
                &args[2..],
            )
        } else {
            let methods = if m == "all" {
                self.warn(line, "`.all()` matches every method; emitted as GET");
                vec!["GET".to_string()]
            } else {
                vec![m.to_ascii_uppercase()]
            };
            if let Some(sub) = recv.sub.clone() {
                (methods, vec![sub], args)
            } else {
                let Some(first) = args.first() else { return };
                if self.flavor == Flavor::Express
                    && args.len() == 1
                    && self.str_of(*first).is_some()
                {
                    return; // `app.get('setting')`
                }
                if is_function_node(unwrap(*first)) {
                    return;
                }
                match self.path_list(*first) {
                    Some(ps) => (methods, ps, &args[1..]),
                    None => {
                        self.warn(line, "route path is not statically known; skipped");
                        return;
                    }
                }
            }
        };
        let mw = !handlers.is_empty()
            && handlers
                .iter()
                .all(|a| unwrap(*a).kind() == "call_expression");
        let handler = handlers
            .last()
            .filter(|a| matches!(a.kind(), "identifier" | "member_expression"))
            .map(|a| self.t(*a).to_string());
        let mut query = Vec::new();
        for a in handlers
            .iter()
            .filter(|a| !matches!(a.kind(), "identifier" | "member_expression"))
        {
            collect_query(*a, self.src, &mut query);
        }
        dedup(&mut query);
        for method in methods
            .iter()
            .filter(|x| HTTP_METHODS.contains(&x.as_str()))
        {
            for p in &paths {
                self.g.regs.push(Reg {
                    inst: recv.key.clone(),
                    method: method.clone(),
                    path: p.clone(),
                    line,
                    handler: handler.clone(),
                    query: query.clone(),
                    call: obj.end_byte(),
                    mw,
                });
            }
        }
    }
}

/// Receiver of a chain: instance key plus an Express `app.route('/x')` path.
#[derive(Clone)]
struct Recv {
    key: String,
    sub: Option<String>,
}

fn clean_path(raw: &str, flavor: Flavor) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    if flavor == Flavor::Express {
        let re =
            RE.get_or_init(|| regex::Regex::new(r":([A-Za-z0-9_]+)(\([^)]*\))?[?*+]?").unwrap());
        return re.replace_all(raw, ":$1").into_owned();
    }
    raw.to_string()
}

fn unnamed_param_warning(file: &str, line: u32, raw: &str) -> String {
    format!(
        "{}:{}: path parameter without a recoverable name emitted as `{{{}}}` (raw path: `{}`); please report this pattern",
        file,
        line,
        naming::UNNAMED_PARAM,
        raw
    )
}

/// Comparison key of a registered path: normalized, parameter names erased.
fn path_key(raw: &str, flavor: Flavor) -> String {
    naming::normalize_path(&clean_path(raw, flavor))
        .split('/')
        .map(|s| if s.starts_with('{') { "{}" } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

/// Hono (and Express) cannot tell middleware from handlers syntactically.
/// Rule: a registration (`.on`, `.get`, ..., `.all`) whose handler arguments
/// are ALL call expressions (`adminChecker("X")`, `zValidator(...)`; no
/// identifier, arrow or function handler) and every one of whose paths is
/// also registered by some OTHER registration of the same router instance
/// that is not of that kind (any method, normalized paths compared, param
/// names ignored) is middleware and emits no routes. A call-expression
/// handler on a path nothing else registers, such as
/// `app.get("/reference", apiReference({...}))`, stays a route.
fn drop_middleware_regs(g: &mut FileGraph, flavor: Flavor) {
    let mut real: HashMap<&str, HashSet<String>> = HashMap::new();
    for r in g.regs.iter().filter(|r| !r.mw) {
        real.entry(r.inst.as_str())
            .or_default()
            .insert(path_key(&r.path, flavor));
    }
    let mut covered: HashMap<(String, usize), bool> = HashMap::new();
    for r in g.regs.iter().filter(|r| r.mw) {
        let ok = real
            .get(r.inst.as_str())
            .is_some_and(|set| set.contains(&path_key(&r.path, flavor)));
        *covered.entry((r.inst.clone(), r.call)).or_insert(true) &= ok;
    }
    g.regs
        .retain(|r| !(r.mw && covered[&(r.inst.clone(), r.call)]));
}

struct Entry {
    path: std::path::PathBuf,
    rel: String,
    src: Option<String>,
    strong: bool,
}

/// Names and module stems that identify calls of the known factories in
/// files that do not mention Hono themselves.
fn factory_needles(factories: &HashSet<(usize, String)>, entries: &[Entry]) -> HashSet<String> {
    let mut out = HashSet::new();
    for (fi, name) in factories {
        if name != "<default>" && name.len() >= 4 {
            out.insert(name.clone());
        }
        let p = Path::new(&entries[*fi].rel);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let stem = if stem == "index" {
            p.parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("")
        } else {
            stem
        };
        if stem.len() >= 4 {
            out.insert(stem.to_string());
        }
    }
    out
}

/// Scan Hono / Express sources. Returns routes and warnings.
pub fn scan_graphs(
    flavor: Flavor,
    root: &Path,
    files: &[std::path::PathBuf],
) -> Result<(Vec<Route>, Vec<String>)> {
    let hono = flavor == Flavor::Hono;
    let mut parsers = Parsers::new()?;
    let mut warnings = Vec::new();

    // Read everything once. Express keeps only files that mention it; Hono
    // keeps all (factories can live behind any name).
    let mut entries: Vec<Entry> = Vec::new();
    for f in files.iter().filter(|f| is_ts_source(f)) {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        let lower = src.to_ascii_lowercase();
        let strong = if hono {
            ["hono", "openapi", "createroute"]
                .iter()
                .any(|k| lower.contains(k))
        } else {
            lower.contains("express")
        };
        entries.push(Entry {
            path: f.clone(),
            rel: rel_path(root, f),
            src: (strong || hono).then_some(src),
            strong,
        });
    }
    let n = entries.len();
    let mod_inputs: Vec<(&Path, &str)> = entries
        .iter()
        .map(|e| (e.path.as_path(), e.rel.as_str()))
        .collect();
    let mods = Modules::new(&mod_inputs);
    let mut decls: Vec<FileDecl> = entries
        .iter()
        .map(|e| FileDecl {
            rel: e.rel.clone(),
            ..Default::default()
        })
        .collect();
    let mut trees: Vec<Option<Tree>> = (0..n).map(|_| None).collect();
    let mut relevant: Vec<bool> = entries.iter().map(|e| e.strong).collect();
    let mut decl_done: Vec<bool> = vec![false; n];
    let mut factories: HashSet<(usize, String)> = HashSet::new();

    // Declarations (imports, exports, consts, route definitions) come from
    // one parse of every file. Trees are kept for relevant files only; for
    // Hono that set grows with files that call known factories until nothing
    // changes (such a file is parsed a second time, once).
    loop {
        for i in 0..n {
            let need_decl = !decl_done[i];
            let need_tree = relevant[i] && trees[i].is_none();
            if !need_decl && !need_tree {
                continue;
            }
            decl_done[i] = true;
            let Some(src) = entries[i].src.as_ref() else {
                continue;
            };
            let Some(tree) = parsers.parse(&entries[i].path, src) else {
                continue;
            };
            if need_decl {
                let mut d = FileDecl {
                    rel: entries[i].rel.clone(),
                    ..Default::default()
                };
                decl_walk(tree.root_node(), src, hono, &mut d);
                decls[i] = d;
            }
            // Trees are kept only for files that can hold routes or factories.
            if relevant[i] {
                trees[i] = Some(tree);
            }
        }
        if !hono {
            break;
        }
        loop {
            let res = Resolver {
                decls: &decls,
                mods: &mods,
                factories: std::mem::take(&mut factories),
                ext: true,
                hono_types: vec![],
            };
            let mut new = Vec::new();
            for (fi, tree) in trees.iter().enumerate() {
                let (Some(tree), Some(src)) = (tree, entries[fi].src.as_ref()) else {
                    continue;
                };
                let mut funcs = Vec::new();
                let mut c = tree.root_node().walk();
                for top in tree.root_node().named_children(&mut c) {
                    top_funcs(top, src, &mut funcs);
                }
                for (name, f) in funcs {
                    if !res.factories.contains(&(fi, name.clone()))
                        && res.func_returns_hono(f, fi, src)
                    {
                        new.push((fi, name));
                    }
                }
            }
            factories = res.factories;
            if new.is_empty() {
                break;
            }
            factories.extend(new);
        }
        let needles = factory_needles(&factories, &entries);
        let mut changed = false;
        if !needles.is_empty() {
            for i in 0..n {
                if relevant[i] {
                    continue;
                }
                if let Some(s) = &entries[i].src {
                    if needles.iter().any(|nd| s.contains(nd.as_str())) {
                        relevant[i] = true;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let hono_types: Vec<String> = decls
        .iter()
        .flat_map(|d| d.type_aliases.iter().cloned())
        .collect();
    let res = Resolver {
        decls: &decls,
        mods: &mods,
        factories,
        ext: hono,
        hono_types,
    };

    let mut graphs: Vec<FileGraph> = Vec::new();
    let mut used: HashSet<(usize, usize)> = HashSet::new();
    for (i, e) in entries.iter().enumerate() {
        let (Some(src), Some(tree)) = (e.src.as_ref(), trees[i].as_ref()) else {
            graphs.push(FileGraph {
                rel: e.rel.clone(),
                ..Default::default()
            });
            continue;
        };
        let mut fcalls = HashSet::new();
        if hono && !res.factories.is_empty() {
            factory_calls(tree.root_node(), src, &res, i, &mut fcalls);
        }
        let mut b = Builder {
            src,
            flavor,
            fi: i,
            res: &res,
            g: FileGraph {
                rel: e.rel.clone(),
                ..Default::default()
            },
            fcalls,
            creations: HashMap::new(),
            derived: HashMap::new(),
            arrays: HashMap::new(),
            loop_vars: HashMap::new(),
            used: HashSet::new(),
            linked: HashSet::new(),
            unresolved: vec![],
            warnings: vec![],
        };
        b.collect(tree.root_node());
        if hono {
            b.collect_fn_returns(tree.root_node());
        }
        b.visit(tree.root_node());
        if let Some(first) = b.unresolved.first().copied() {
            let cnt = b.unresolved.len();
            b.warn(
                first,
                &format!(
                    "{} `.openapi()` route reference(s) could not be resolved (first here)",
                    cnt
                ),
            );
        }
        warnings.extend(b.warnings);
        used.extend(b.used);
        drop_middleware_regs(&mut b.g, flavor);
        graphs.push(b.g);
    }

    let resolve_import = |gi: usize, spec: &str, imp: &str| -> Option<(usize, String)> {
        let fi = mods.resolve(gi, spec)?;
        let mut cur = (fi, imp.to_string());
        for _ in 0..7 {
            let g = &graphs[cur.0];
            let key = if cur.1 == "default" {
                g.default_export
                    .clone()
                    .filter(|k| g.insts.contains_key(k))
                    .or_else(|| {
                        (g.insts.len() == 1).then(|| g.insts.keys().next().unwrap().clone())
                    })
            } else {
                g.named
                    .get(&cur.1)
                    .cloned()
                    .filter(|k| g.insts.contains_key(k))
                    .or_else(|| g.insts.contains_key(&cur.1).then(|| cur.1.clone()))
            };
            if let Some(k) = key {
                return Some((cur.0, k));
            }
            if !hono {
                return None;
            }
            // Follow re-exports to the declaring file.
            let next = res.export_in(cur.0, &cur.1, 0)?;
            if next == cur {
                return None;
            }
            cur = next;
        }
        None
    };

    let mut children: HashMap<Node2, Vec<(String, Node2)>> = HashMap::new();
    let mut mounted: HashSet<Node2> = HashSet::new();
    // Instances a factory function returns: reachable only through its call sites.
    let mut factory_internal: HashSet<Node2> = HashSet::new();
    for (gi, g) in graphs.iter().enumerate() {
        for keys in g.fn_ret.values() {
            for k in keys {
                factory_internal.insert((gi, k.clone()));
            }
        }
    }
    // (file, name, spec) -> (cause, lines) of mounts that did not resolve.
    let mut unresolved_mounts: IndexMap<(usize, String, String), (String, Vec<u32>)> =
        IndexMap::new();
    for (gi, g) in graphs.iter().enumerate() {
        for m in &g.mounts {
            if let ChildRef::Factory(callee) = &m.child {
                let target_fn = if g.fn_ret.contains_key(callee) {
                    Some((gi, callee.clone()))
                } else {
                    g.imports
                        .get(callee)
                        .and_then(|(spec, imp)| res.resolve_import(gi, spec, imp))
                };
                let keys = target_fn
                    .and_then(|(fj, local)| graphs[fj].fn_ret.get(&local).map(|k| (fj, k.clone())));
                if let Some((fj, keys)) = keys {
                    for k in keys {
                        let t = (fj, k);
                        if t.0 == gi && t.1 == m.parent {
                            continue;
                        }
                        mounted.insert(t.clone());
                        children
                            .entry((gi, m.parent.clone()))
                            .or_default()
                            .push((String::new(), t));
                    }
                }
                continue;
            }
            let target = match &m.child {
                ChildRef::Factory(_) => None,
                ChildRef::Key(k) => Some((gi, k.clone())),
                ChildRef::Name(n) => {
                    if g.insts.contains_key(n) {
                        Some((gi, n.clone()))
                    } else if let Some((spec, imp)) = g.imports.get(n) {
                        let r = resolve_import(gi, spec, imp);
                        if r.is_none() && hono && mods.is_local_spec(gi, spec) {
                            let cause = match mods.resolve(gi, spec) {
                                None => format!(
                                    "the import `{}` does not resolve to a scanned file (unresolved import alias? check tsconfig `paths`/`baseUrl`)",
                                    spec
                                ),
                                Some(fj) => format!(
                                    "`{}` exported from {} is not a recognised Hono instance (is it created by an unrecognised factory function?)",
                                    imp, graphs[fj].rel
                                ),
                            };
                            let e = unresolved_mounts
                                .entry((gi, n.clone(), spec.clone()))
                                .or_insert((cause, vec![]));
                            if !e.1.contains(&m.line) {
                                e.1.push(m.line);
                            }
                        }
                        r
                    } else {
                        None
                    }
                }
                ChildRef::Require(spec, imp) => {
                    resolve_import(gi, spec, imp.as_deref().unwrap_or("default"))
                }
                ChildRef::NsMember(ns, prop) => match g.imports.get(ns) {
                    Some((spec, imp)) if imp == "*" => resolve_import(gi, spec, prop),
                    _ => None,
                },
            };
            if let Some(t) = target {
                if t.0 == gi && t.1 == m.parent {
                    continue;
                }
                mounted.insert(t.clone());
                children
                    .entry((gi, m.parent.clone()))
                    .or_default()
                    .push((m.prefix.clone(), t));
            }
        }
    }

    for ((gi, name, spec), (cause, lines)) in &unresolved_mounts {
        let mut msg = format!(
            "{}:{}: could not resolve mounted app `{}` ({}): {}",
            graphs[*gi].rel, lines[0], name, spec, cause
        );
        if lines.len() > 1 {
            let more: Vec<String> = lines[1..].iter().map(|l| l.to_string()).collect();
            msg.push_str(&format!(" (also mounted at line {})", more.join(", ")));
        }
        warnings.push(msg);
    }

    let mut out = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        graphs: &[FileGraph],
        children: &HashMap<Node2, Vec<(String, Node2)>>,
        node: &Node2,
        prefix: &str,
        stack: &mut Vec<Node2>,
        flavor: Flavor,
        out: &mut Vec<Route>,
        warnings: &mut Vec<String>,
    ) {
        let g = &graphs[node.0];
        let base = format!("{}/{}", prefix, g.insts[&node.1].base);
        for r in g.regs.iter().filter(|r| r.inst == node.1) {
            let raw = clean_path(&format!("{}/{}", base, r.path), flavor);
            let (full, unnamed) = naming::normalize_path_checked(&raw);
            if unnamed {
                warnings.push(unnamed_param_warning(&g.rel, r.line, &r.path));
            }
            out.push(Route {
                method: r.method.clone(),
                path: full,
                file: g.rel.clone(),
                line: r.line,
                handler: r.handler.clone(),
                query_params: r.query.clone(),
                framework: flavor.name().to_string(),
            });
        }
        stack.push(node.clone());
        if let Some(cs) = children.get(node) {
            for (p, child) in cs {
                if stack.contains(child) {
                    continue;
                }
                dfs(
                    graphs,
                    children,
                    child,
                    &format!("{}/{}", base, p),
                    stack,
                    flavor,
                    out,
                    warnings,
                );
            }
        }
        stack.pop();
    }
    for (gi, g) in graphs.iter().enumerate() {
        for key in g.insts.keys() {
            let node = (gi, key.clone());
            if !mounted.contains(&node) {
                if factory_internal.contains(&node) {
                    let n = g.regs.iter().filter(|r| r.inst == node.1).count();
                    if n > 0 {
                        let first = g.regs.iter().find(|r| r.inst == node.1).unwrap().line;
                        warnings.push(format!(
                            "{}:{}: {} route(s) registered on an app created inside a factory function that is never called were not emitted",
                            g.rel, first, n
                        ));
                    }
                    continue;
                }
                dfs(
                    &graphs,
                    &children,
                    &node,
                    "",
                    &mut Vec::new(),
                    flavor,
                    &mut out,
                    &mut warnings,
                );
            }
        }
    }

    // Fallback: createRoute() definitions never consumed by `.openapi()`
    // are still emitted, with the path as written.
    if hono {
        let mut emitted = 0usize;
        let mut seen: IndexMap<(String, String), Vec<String>> = IndexMap::new();
        for (fi, d) in decls.iter().enumerate() {
            for def in &d.all_defs {
                if used.contains(&(fi, def.key)) {
                    continue;
                }
                let method = res.eval(&def.method, fi).map(|m| m.to_ascii_uppercase());
                match (method, res.eval(&def.path, fi)) {
                    (Some(m), Some(p)) if HTTP_METHODS.contains(&m.as_str()) => {
                        let (path, unnamed) = naming::normalize_path_checked(&p);
                        if unnamed {
                            warnings.push(unnamed_param_warning(&d.rel, def.line, &p));
                        }
                        seen.entry((m.clone(), path.clone()))
                            .or_default()
                            .push(format!("{}:{}", d.rel, def.line));
                        out.push(Route {
                            method: m,
                            path,
                            file: d.rel.clone(),
                            line: def.line,
                            handler: None,
                            query_params: vec![],
                            framework: flavor.name().to_string(),
                        });
                        emitted += 1;
                    }
                    _ => warnings.push(format!(
                        "{}:{}: createRoute() method or path is not statically known; skipped",
                        d.rel, def.line
                    )),
                }
            }
        }
        if emitted > 0 {
            let no_app = graphs.iter().all(|g| g.insts.is_empty());
            let mut msg = format!(
                "{} createRoute() definition(s) are not connected to any app through `.openapi()`; emitted with the path as written (no mount prefixes)",
                emitted
            );
            if no_app {
                msg.push_str(
                    ". No Hono app instance (`new Hono()` / `new OpenAPIHono()`) was found at all: the app is probably created by a factory function that was not recognised (a function that returns `new Hono()` / `new OpenAPIHono()`, directly or inside a wrapper call), or imported through an alias that does not resolve (check tsconfig `paths`)",
                );
            }
            warnings.push(msg);
        }
        for ((m, p), locs) in seen.iter().filter(|(_, l)| l.len() > 1) {
            let shown: Vec<&str> = locs.iter().take(6).map(|s| s.as_str()).collect();
            warnings.push(format!(
                "{} createRoute() definitions share `{} {}` ({}{}); they are reported as one endpoint",
                locs.len(),
                m,
                p,
                shown.join(", "),
                if locs.len() > 6 { ", ..." } else { "" }
            ));
        }
    }
    dedup(&mut warnings);
    Ok((out, warnings))
}
