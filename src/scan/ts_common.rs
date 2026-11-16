//! Shared helpers for the TypeScript / JavaScript scanners (Next.js, Hono,
//! Express) plus the "app graph" engine used by Hono and Express: it finds
//! app / router instances, route registrations and mounts (`app.route`,
//! `app.use`) across files and composes the final paths.

use super::Route;
use crate::naming;
use anyhow::Result;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::path::Path;
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

/// Resolve an import specifier to the index of a known file.
fn resolve_module(from_rel: &str, spec: &str, files: &HashMap<String, usize>) -> Option<usize> {
    let bases: Vec<String> =
        if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
            let dir = from_rel.rsplit_once('/').map(|x| x.0).unwrap_or("");
            vec![norm_join(dir, spec)]
        } else {
            let rest = spec
                .strip_prefix("@/")
                .or_else(|| spec.strip_prefix("~/"))?;
            vec![format!("src/{}", rest), rest.to_string()]
        };
    for base in bases {
        if let Some(i) = files.get(&base) {
            return Some(*i);
        }
        let stripped = [".js", ".mjs", ".cjs", ".jsx"]
            .iter()
            .find_map(|s| base.strip_suffix(s));
        for b in std::iter::once(base.as_str()).chain(stripped) {
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
    }
    None
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
    /// local name -> (module specifier, imported name or "default")
    imports: HashMap<String, (String, String)>,
    default_export: Option<String>,
    named: HashMap<String, String>,
}

#[derive(Clone)]
struct Recv {
    key: String,
    /// Path of an Express `app.route('/x')` builder.
    sub: Option<String>,
}

type Node2 = (usize, String);

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

struct Builder<'a> {
    src: &'a str,
    flavor: Flavor,
    g: FileGraph,
    consts: HashMap<String, String>,
    route_defs: HashMap<String, (String, String)>,
    creations: HashMap<usize, String>,
    derived: HashMap<usize, String>,
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
        let u = unwrap(n);
        if let Some(s) = str_lit(u, self.src) {
            return Some(s);
        }
        if u.kind() == "identifier" {
            return self.consts.get(self.t(u)).cloned();
        }
        None
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

    fn route_def_of(&self, n: Node<'a>) -> Option<(String, String)> {
        let u = unwrap(n);
        match u.kind() {
            "identifier" => self.route_defs.get(self.t(u)).cloned(),
            "call_expression" => {
                let f = u.child_by_field_name("function")?;
                if self.t(f) != "createRoute" {
                    return None;
                }
                let obj = u.child_by_field_name("arguments")?.named_child(0)?;
                let method = self.str_of(obj_prop(obj, "method", self.src)?)?;
                let path = self.str_of(obj_prop(obj, "path", self.src)?)?;
                Some((method.to_ascii_uppercase(), path))
            }
            _ => None,
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
        if let Some(s) = str_lit(v, self.src) {
            self.consts.insert(nm.clone(), s);
        }
        if let Some(rd) = self.route_def_of(v) {
            self.route_defs.insert(nm.clone(), rd);
        }
        if let Some((spec, imported)) = self.require_of(v) {
            self.g.imports.insert(
                nm.clone(),
                (spec, imported.unwrap_or_else(|| "default".into())),
            );
        }
        if let Some(base) = self.chain_base(v) {
            if self.is_creation(base) {
                self.creations.insert(base.start_byte(), nm.clone());
                self.ensure_inst(&nm);
                return;
            }
        }
        // `const v1 = app.basePath('/v1')`
        let u = unwrap(v);
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
            Flavor::Hono => ty.contains("Hono"),
            Flavor::Express => {
                ty.contains("Express") || ty.contains("Router") || ty.contains("Application")
            }
        };
        if hit {
            let nm = self.t(pat).to_string();
            self.ensure_inst(&nm);
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
        if n.kind() == "call_expression" {
            self.call(n);
        }
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            self.visit(ch);
        }
    }

    fn path_list(&self, n: Node<'a>) -> Option<Vec<String>> {
        let u = unwrap(n);
        if u.kind() == "array" {
            let mut c = u.walk();
            let v: Vec<String> = u
                .named_children(&mut c)
                .filter_map(|e| self.str_of(e))
                .collect();
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
                let (spec, _) = self.require_of(u)?;
                let imp = u
                    .child_by_field_name("property")
                    .map(|p| self.t(p).to_string());
                Some(ChildRef::Require(spec, imp))
            }
            _ => None,
        }
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
                let Some((method, path)) = args.first().and_then(|a| self.route_def_of(*a)) else {
                    return;
                };
                let handler = args
                    .get(1)
                    .filter(|a| a.kind() == "identifier")
                    .map(|a| self.t(*a).to_string());
                self.g.regs.push(Reg {
                    inst: recv.key,
                    method,
                    path,
                    line,
                    handler,
                    query: vec![],
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
                if let Some(child) = self.child_ref(args[1]) {
                    self.g.mounts.push(Mount {
                        parent: recv.key,
                        prefix,
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
                });
            }
        }
    }
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

/// Scan Hono / Express sources. Returns routes and warnings.
pub fn scan_graphs(
    flavor: Flavor,
    root: &Path,
    files: &[std::path::PathBuf],
) -> Result<(Vec<Route>, Vec<String>)> {
    let needle = match flavor {
        Flavor::Hono => "hono",
        Flavor::Express => "express",
    };
    let mut parsers = Parsers::new()?;
    let mut graphs: Vec<FileGraph> = Vec::new();
    let mut warnings = Vec::new();
    for f in files.iter().filter(|f| is_ts_source(f)) {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        if !src.to_ascii_lowercase().contains(needle) {
            continue;
        }
        let Some(tree) = parsers.parse(f, &src) else {
            continue;
        };
        let mut b = Builder {
            src: &src,
            flavor,
            g: FileGraph {
                rel: rel_path(root, f),
                ..Default::default()
            },
            consts: HashMap::new(),
            route_defs: HashMap::new(),
            creations: HashMap::new(),
            derived: HashMap::new(),
            warnings: vec![],
        };
        b.collect(tree.root_node());
        b.visit(tree.root_node());
        warnings.extend(b.warnings);
        graphs.push(b.g);
    }
    let by_rel: HashMap<String, usize> = graphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.rel.clone(), i))
        .collect();

    let resolve_import = |gi: usize, spec: &str, imp: &str| -> Option<(usize, String)> {
        let fi = resolve_module(&graphs[gi].rel, spec, &by_rel)?;
        let g = &graphs[fi];
        let key = if imp == "default" {
            g.default_export
                .clone()
                .filter(|k| g.insts.contains_key(k))
                .or_else(|| (g.insts.len() == 1).then(|| g.insts.keys().next().unwrap().clone()))
        } else {
            g.named
                .get(imp)
                .cloned()
                .filter(|k| g.insts.contains_key(k))
                .or_else(|| g.insts.contains_key(imp).then(|| imp.to_string()))
        }?;
        Some((fi, key))
    };

    let mut children: HashMap<Node2, Vec<(String, Node2)>> = HashMap::new();
    let mut mounted: HashSet<Node2> = HashSet::new();
    for (gi, g) in graphs.iter().enumerate() {
        for m in &g.mounts {
            let target = match &m.child {
                ChildRef::Key(k) => Some((gi, k.clone())),
                ChildRef::Name(n) => {
                    if g.insts.contains_key(n) {
                        Some((gi, n.clone()))
                    } else if let Some((spec, imp)) = g.imports.get(n) {
                        let r = resolve_import(gi, spec, imp);
                        if r.is_none()
                            && flavor == Flavor::Hono
                            && (spec.starts_with("./") || spec.starts_with("../"))
                        {
                            warnings.push(format!(
                                "{}:{}: could not resolve mounted app `{}` ({})",
                                g.rel, m.line, n, spec
                            ));
                        }
                        r
                    } else {
                        None
                    }
                }
                ChildRef::Require(spec, imp) => {
                    resolve_import(gi, spec, imp.as_deref().unwrap_or("default"))
                }
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
    ) {
        let g = &graphs[node.0];
        let base = format!("{}/{}", prefix, g.insts[&node.1].base);
        for r in g.regs.iter().filter(|r| r.inst == node.1) {
            let full = naming::normalize_path(&clean_path(&format!("{}/{}", base, r.path), flavor));
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
                );
            }
        }
        stack.pop();
    }
    for (gi, g) in graphs.iter().enumerate() {
        for key in g.insts.keys() {
            let node = (gi, key.clone());
            if !mounted.contains(&node) {
                dfs(
                    &graphs,
                    &children,
                    &node,
                    "",
                    &mut Vec::new(),
                    flavor,
                    &mut out,
                );
            }
        }
    }
    Ok((out, warnings))
}
