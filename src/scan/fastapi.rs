//! FastAPI route scanner (tree-sitter based).
//!
//! Pass 1 parses every `.py` file once and extracts, per file: imports, router
//! instances (`FastAPI()` / `APIRouter(prefix=...)`), module-level string
//! constants, decorated route functions, `add_api_route` calls and
//! `include_router` calls.  Pass 2 resolves routers across modules, composes
//! the prefixes recursively and emits one [`Route`] per (method, full path).
//!
//! Limitations: prefixes / paths that are not string literals, f-strings,
//! `+` concatenations or simple module-level constants are treated as dynamic
//! and the affected routes are skipped.

use super::Route;
use crate::naming;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

const FRAMEWORK: &str = "fastapi";
const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];
const MAX_DEPTH: usize = 400;
const SKIP_DIRS: &[&str] = &[
    ".venv",
    "venv",
    "site-packages",
    "node_modules",
    "__pycache__",
    ".git",
    ".tox",
];
const SCALARS: &[&str] = &[
    "int",
    "str",
    "float",
    "bool",
    "bytes",
    "UUID",
    "datetime",
    "date",
    "time",
    "timedelta",
    "Decimal",
];
const CONTAINERS: &[&str] = &[
    "Optional",
    "List",
    "list",
    "Set",
    "set",
    "FrozenSet",
    "frozenset",
    "Sequence",
    "Union",
    "Tuple",
    "tuple",
    "Iterable",
];
const EXCLUDED_DEFAULTS: &[&str] = &[
    "Body", "Depends", "Header", "Cookie", "Form", "File", "Security", "Path",
];

/// A (possibly dynamic) string built from literals and references to module
/// level constants.  `None` at use sites means "dynamic".
#[derive(Clone, Debug)]
enum Part {
    Lit(String),
    Ref(String),
}
type PStr = Vec<Part>;

struct Import {
    level: usize,
    module: String,
    sym: Option<String>,
}

struct RouteDecl {
    owner: String,
    methods: Vec<String>,
    path: Option<PStr>,
    line: u32,
    handler: Option<String>,
    params: Vec<String>,
}

struct Include {
    parent: String,
    child: String,
    prefix: Option<PStr>,
    line: u32,
    /// `app.mount(path, sub_app)` instead of `include_router`
    mount: bool,
}

/// A `FastAPI()` / `APIRouter()` instance.
struct Inst {
    /// own prefix (`None` = dynamic)
    prefix: Option<PStr>,
    is_app: bool,
    /// created inside a function (the instance key is `func::name`)
    local: bool,
    line: u32,
    /// `Some((function, param))` for a function parameter used as an app /
    /// router (`def setup(app): app.include_router(...)`); resolved through
    /// the call sites of `function`.
    param_of: Option<(String, String)>,
}

/// A call `callee(args...)`; only arguments that are plain names / attributes are kept.
struct CallRec {
    callee: String,
    args: Vec<String>,
    kwargs: Vec<(String, String)>,
    line: u32,
}

/// A websocket route or a mount that is not turned into an endpoint.
struct Skipped {
    owner: String,
    path: String,
    line: u32,
}

#[derive(Default)]
struct FileInfo {
    rel: String,
    imports: HashMap<String, Import>,
    instances: HashMap<String, Inst>,
    /// local instances (`func::name`) that the function returns (app factories)
    returned: HashSet<String>,
    /// top-level function -> the local instance it returns (`create_app` -> `create_app::app`)
    returned_fns: HashMap<String, String>,
    /// module-level `NAME = factory(...)`: NAME -> callee text
    aliases: HashMap<String, String>,
    /// module-level `NAME = (a.router, b.router)` lists, for `for r in NAME:` loops
    lists: HashMap<String, Vec<String>>,
    consts: HashMap<String, PStr>,
    routes: Vec<RouteDecl>,
    includes: Vec<Include>,
    skipped: Vec<Skipped>,
    /// parameter names of the functions that use a parameter as an app / router
    fn_params: HashMap<String, Vec<String>>,
    calls: Vec<CallRec>,
}

struct Parsed {
    rel: String,
    src: String,
    tree: tree_sitter::Tree,
}

type Key = (usize, String);

pub fn scan(root: &Path, files: &[PathBuf]) -> Result<Vec<Route>> {
    Ok(scan_with_warnings(root, files)?.0)
}

/// Like [`scan`], and also returns human-readable warnings (unreachable
/// routers, shadowed duplicates, skipped websockets / mounts, ...).
pub fn scan_with_warnings(root: &Path, files: &[PathBuf]) -> Result<(Vec<Route>, Vec<String>)> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_python::LANGUAGE.into())?;

    let mut parsed: Vec<Parsed> = Vec::new();
    for f in files {
        if f.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        let rel = rel_path(root, f);
        if rel.split('/').any(|c| SKIP_DIRS.contains(&c)) {
            continue;
        }
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let src = String::from_utf8_lossy(&bytes).into_owned();
        if let Some(tree) = parser.parse(&src, None) {
            parsed.push(Parsed { rel, src, tree });
        }
    }
    parsed.sort_by(|a, b| a.rel.cmp(&b.rel));

    // Phase A: enum-like classes (their parameters are query-able simple types).
    let mut enums: HashSet<String> = HashSet::new();
    for p in &parsed {
        collect_enums(p.tree.root_node(), &p.src, &mut enums, 0);
    }

    // Phase B: per-file extraction.
    let infos: Vec<FileInfo> = parsed.iter().map(|p| extract(p, &enums)).collect();

    let cx = Resolver::new(&infos);
    Ok(cx.emit())
}

fn rel_path(root: &Path, f: &Path) -> String {
    let rel = f.strip_prefix(root).unwrap_or(f);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn text<'a>(n: Node, src: &'a str) -> &'a str {
    src.get(n.byte_range()).unwrap_or("")
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

fn collect_enums(node: Node, src: &str, out: &mut HashSet<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    if node.kind() == "class_definition" {
        if let (Some(name), Some(sup)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("superclasses"),
        ) {
            let mut c = sup.walk();
            let is_enum = sup.named_children(&mut c).any(|b| {
                let t = tail_name(text(b, src));
                matches!(t, "Enum" | "IntEnum" | "StrEnum" | "Flag" | "IntFlag")
            });
            if is_enum {
                out.insert(text(name, src).to_string());
            }
        }
    }
    let mut c = node.walk();
    for ch in node.children(&mut c) {
        collect_enums(ch, src, out, depth + 1);
    }
}

fn tail_name(s: &str) -> &str {
    s.rsplit('.').next().unwrap_or(s).trim()
}

fn extract(p: &Parsed, enums: &HashSet<String>) -> FileInfo {
    let mut info = FileInfo {
        rel: p.rel.clone(),
        ..Default::default()
    };
    let root = p.tree.root_node();
    let src = p.src.as_str();

    // Top-level functions (for add_api_route) and constants.
    let mut funcs: HashMap<String, Node> = HashMap::new();
    let mut c = root.walk();
    for ch in root.children(&mut c) {
        let def = if ch.kind() == "decorated_definition" {
            ch.child_by_field_name("definition")
        } else {
            Some(ch)
        };
        if let Some(d) = def {
            if d.kind() == "function_definition" {
                if let Some(n) = d.child_by_field_name("name") {
                    funcs.insert(text(n, src).to_string(), d);
                }
            }
        }
        if ch.kind() == "expression_statement" {
            if let Some(a) = ch.named_child(0).filter(|a| a.kind() == "assignment") {
                if let (Some(l), Some(r)) = (
                    a.child_by_field_name("left"),
                    a.child_by_field_name("right"),
                ) {
                    if l.kind() == "identifier" {
                        if let Some(v) = eval_pstr(r, src) {
                            info.consts.insert(text(l, src).to_string(), v);
                        } else if matches!(r.kind(), "list" | "tuple" | "parenthesized_expression")
                        {
                            let mut v = vec![];
                            iter_candidates(r, src, &mut v, 0);
                            if !v.is_empty() {
                                info.lists.insert(text(l, src).to_string(), v);
                            }
                        }
                    }
                }
            }
        }
    }

    // Imports first (needed to recognise aliased constructors).
    collect_imports(root, src, &mut info, 0);
    let mut cx = Extract {
        src,
        enums,
        funcs: &funcs,
        scope: vec![],
        params: vec![],
        loops: vec![],
    };
    cx.walk(root, &mut info, 0);
    info
}

fn collect_imports(node: Node, src: &str, info: &mut FileInfo, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match node.kind() {
        "import_statement" => {
            let mut c = node.walk();
            for n in node.children_by_field_name("name", &mut c) {
                match n.kind() {
                    "dotted_name" => {
                        let full = text(n, src).to_string();
                        let head = full.split('.').next().unwrap_or("").to_string();
                        info.imports.insert(
                            head.clone(),
                            Import {
                                level: 0,
                                module: head,
                                sym: None,
                            },
                        );
                    }
                    "aliased_import" => {
                        if let (Some(nm), Some(al)) = (
                            n.child_by_field_name("name"),
                            n.child_by_field_name("alias"),
                        ) {
                            info.imports.insert(
                                text(al, src).to_string(),
                                Import {
                                    level: 0,
                                    module: text(nm, src).to_string(),
                                    sym: None,
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        "import_from_statement" => {
            let (mut level, mut module) = (0usize, String::new());
            if let Some(m) = node.child_by_field_name("module_name") {
                if m.kind() == "relative_import" {
                    let mut c = m.walk();
                    for ch in m.children(&mut c) {
                        match ch.kind() {
                            "import_prefix" => {
                                level = text(ch, src).chars().filter(|c| *c == '.').count()
                            }
                            "dotted_name" => module = text(ch, src).to_string(),
                            _ => {}
                        }
                    }
                } else {
                    module = text(m, src).to_string();
                }
            }
            let mut c = node.walk();
            for n in node.children_by_field_name("name", &mut c) {
                let (name, alias) = match n.kind() {
                    "dotted_name" => (text(n, src).to_string(), None),
                    "aliased_import" => (
                        n.child_by_field_name("name")
                            .map(|x| text(x, src).to_string())
                            .unwrap_or_default(),
                        n.child_by_field_name("alias")
                            .map(|x| text(x, src).to_string()),
                    ),
                    _ => continue,
                };
                if name.is_empty() {
                    continue;
                }
                let local = alias.unwrap_or_else(|| name.clone());
                info.imports.insert(
                    local,
                    Import {
                        level,
                        module: module.clone(),
                        sym: Some(name),
                    },
                );
            }
            return;
        }
        _ => {}
    }
    let mut c = node.walk();
    for ch in node.children(&mut c) {
        collect_imports(ch, src, info, depth + 1);
    }
}

struct Extract<'a, 't> {
    src: &'a str,
    enums: &'a HashSet<String>,
    funcs: &'a HashMap<String, Node<'t>>,
    /// enclosing function names (outermost first)
    scope: Vec<String>,
    /// parameter names of each enclosing function (parallel to `scope`)
    params: Vec<Vec<String>>,
    /// `for NAME in <iterable>` bindings: loop variable -> candidate expressions
    loops: Vec<(String, Vec<String>)>,
}

fn param_names(func: Node, src: &str) -> Vec<String> {
    let mut out = vec![];
    let Some(params) = func.child_by_field_name("parameters") else {
        return out;
    };
    let mut c = params.walk();
    for p in params.named_children(&mut c) {
        let id = match p.kind() {
            "identifier" => Some(p),
            "typed_parameter" => p.named_child(0).filter(|n| n.kind() == "identifier"),
            "default_parameter" | "typed_default_parameter" => p.child_by_field_name("name"),
            _ => None,
        };
        // keep positions aligned with the call arguments
        out.push(id.map(|n| text(n, src).to_string()).unwrap_or_default());
    }
    out
}

/// Identifier / attribute expressions inside a tuple, list or conditional
/// (`for router in (a.router, b.router):`).
fn iter_candidates(n: Node, src: &str, out: &mut Vec<String>, depth: usize) {
    if depth > 20 {
        return;
    }
    match n.kind() {
        "identifier" | "attribute" => out.push(text(n, src).to_string()),
        "conditional_expression" => {
            for i in [0, 2] {
                if let Some(c) = n.named_child(i) {
                    iter_candidates(c, src, out, depth + 1);
                }
            }
        }
        "tuple" | "list" | "set" | "parenthesized_expression" | "expression_list" => {
            let mut c = n.walk();
            for ch in n.named_children(&mut c) {
                iter_candidates(ch, src, out, depth + 1);
            }
        }
        _ => {}
    }
}

impl<'a, 't> Extract<'a, 't> {
    fn walk(&mut self, node: Node<'t>, info: &mut FileInfo, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let (mut pushed_scope, mut pushed_loop) = (false, false);
        match node.kind() {
            "decorated_definition" => self.decorated(node, info),
            "assignment" => self.assignment(node, info),
            "call" => self.call(node, info),
            "return_statement" => self.returned(node, info),
            "function_definition" => {
                if let Some(n) = node.child_by_field_name("name") {
                    self.scope.push(text(n, self.src).to_string());
                    self.params.push(param_names(node, self.src));
                    pushed_scope = true;
                }
            }
            "for_statement" => {
                if let (Some(l), Some(r)) = (
                    node.child_by_field_name("left")
                        .filter(|l| l.kind() == "identifier"),
                    node.child_by_field_name("right"),
                ) {
                    let mut raw = vec![];
                    iter_candidates(r, self.src, &mut raw, 0);
                    let mut cands = vec![];
                    for r in raw {
                        match info.lists.get(&r) {
                            Some(v) => cands.extend(v.iter().cloned()),
                            None => cands.push(r),
                        }
                    }
                    self.loops.push((text(l, self.src).to_string(), cands));
                    pushed_loop = true;
                }
            }
            _ => {}
        }
        let mut c = node.walk();
        for ch in node.children(&mut c) {
            self.walk(ch, info, depth + 1);
        }
        if pushed_scope {
            self.scope.pop();
            self.params.pop();
        }
        if pushed_loop {
            self.loops.pop();
        }
    }

    /// Instance key for a name defined at the current scope.
    fn define_key(&self, name: &str) -> String {
        if self.scope.is_empty() {
            name.to_string()
        } else {
            format!("{}::{}", self.scope.join("::"), name)
        }
    }

    /// Resolve `name` to the innermost local instance (or function parameter)
    /// that defines it, else return it unchanged (module level / imported /
    /// attribute chain).
    fn qualify(&self, name: &str, info: &mut FileInfo, make_param: bool) -> String {
        for i in (1..=self.scope.len()).rev() {
            let key = format!("{}::{}", self.scope[..i].join("::"), name);
            if info.instances.contains_key(&key) {
                return key;
            }
            if make_param && i == 1 && self.params[0].iter().any(|p| p == name) {
                info.fn_params
                    .insert(self.scope[0].clone(), self.params[0].clone());
                info.instances.insert(
                    key.clone(),
                    Inst {
                        prefix: Some(vec![]),
                        is_app: false,
                        local: true,
                        line: 0,
                        param_of: Some((self.scope[0].clone(), name.to_string())),
                    },
                );
                return key;
            }
            if self.params[i - 1].iter().any(|p| p == name) {
                break;
            }
        }
        name.to_string()
    }

    fn record_call(&mut self, node: Node<'t>, func: Node<'t>, info: &mut FileInfo) {
        if !matches!(func.kind(), "identifier" | "attribute") {
            return;
        }
        let Some(args) = arg_list(node) else { return };
        let mut rec = CallRec {
            callee: text(func, self.src).to_string(),
            args: vec![],
            kwargs: vec![],
            line: node.start_position().row as u32 + 1,
        };
        let mut c = args.walk();
        for a in args.named_children(&mut c) {
            let name_of = |n: Node| -> Option<String> {
                matches!(n.kind(), "identifier" | "attribute")
                    .then(|| text(n, self.src).to_string())
            };
            match a.kind() {
                "keyword_argument" => {
                    if let (Some(k), Some(v)) = (
                        a.child_by_field_name("name"),
                        a.child_by_field_name("value").and_then(name_of),
                    ) {
                        rec.kwargs.push((text(k, self.src).to_string(), v));
                    }
                }
                "comment" | "dictionary_splat" | "list_splat" => {}
                _ => rec.args.push(name_of(a).unwrap_or_default()),
            }
        }
        if rec.args.iter().all(|a| a.is_empty()) && rec.kwargs.is_empty() {
            return;
        }
        for a in rec
            .args
            .iter_mut()
            .chain(rec.kwargs.iter_mut().map(|(_, v)| v))
        {
            if !a.is_empty() {
                *a = self.qualify(a, info, false);
            }
        }
        info.calls.push(rec);
    }

    fn returned(&mut self, node: Node<'t>, info: &mut FileInfo) {
        let Some(v) = node.named_child(0).filter(|v| v.kind() == "identifier") else {
            return;
        };
        let key = self.qualify(text(v, self.src), info, false);
        if key.contains("::")
            && info
                .instances
                .get(&key)
                .is_some_and(|i| i.param_of.is_none())
        {
            if self.scope.len() == 1 {
                info.returned_fns.insert(self.scope[0].clone(), key.clone());
            }
            info.returned.insert(key);
        }
    }

    fn ctor_kind(&self, func: Node, info: &FileInfo) -> Option<&'static str> {
        let t = text(func, self.src);
        let mut name = tail_name(t).to_string();
        if func.kind() == "identifier" {
            if let Some(imp) = info.imports.get(t) {
                if let Some(s) = &imp.sym {
                    name = s.clone();
                }
            }
        }
        match name.as_str() {
            "FastAPI" => Some("app"),
            "APIRouter" => Some("router"),
            _ => None,
        }
    }

    fn assignment(&mut self, node: Node<'t>, info: &mut FileInfo) {
        let (Some(l), Some(r)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            return;
        };
        if l.kind() != "identifier" || r.kind() != "call" {
            return;
        }
        let Some(func) = r.child_by_field_name("function") else {
            return;
        };
        let Some(kind) = self.ctor_kind(func, info) else {
            if self.scope.is_empty() && matches!(func.kind(), "identifier" | "attribute") {
                info.aliases.insert(
                    text(l, self.src).to_string(),
                    text(func, self.src).to_string(),
                );
            }
            return;
        };
        let prefix = if kind == "router" {
            match kwarg(r, self.src, "prefix") {
                None => Some(vec![]),
                Some(v) => eval_pstr(v, self.src),
            }
        } else {
            Some(vec![])
        };
        let key = self.define_key(text(l, self.src));
        info.instances.insert(
            key,
            Inst {
                prefix,
                is_app: kind == "app",
                local: !self.scope.is_empty(),
                line: node.start_position().row as u32 + 1,
                param_of: None,
            },
        );
    }

    fn call(&mut self, node: Node<'t>, info: &mut FileInfo) {
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
        self.record_call(node, func, info);
        if func.kind() != "attribute" {
            return;
        }
        let (Some(obj), Some(attr)) = (
            func.child_by_field_name("object"),
            func.child_by_field_name("attribute"),
        ) else {
            return;
        };
        let attr = text(attr, self.src);
        if !matches!(
            attr,
            "include_router"
                | "mount"
                | "add_websocket_route"
                | "add_api_websocket_route"
                | "add_api_route"
        ) {
            return;
        }
        let owner = self.qualify(text(obj, self.src), info, true);
        let line = node.start_position().row as u32 + 1;
        match attr {
            "include_router" => {
                let child = positional(node, 0).or_else(|| kwarg(node, self.src, "router"));
                let Some(child) = child else { return };
                let prefix = match kwarg(node, self.src, "prefix") {
                    None => Some(vec![]),
                    Some(v) => eval_pstr(v, self.src),
                };
                let ctext = text(child, self.src).to_string();
                let looped = if child.kind() == "identifier" {
                    self.loops.iter().rev().find(|(n, _)| *n == ctext)
                } else {
                    None
                };
                let children: Vec<String> = match looped {
                    Some((_, c)) if !c.is_empty() => c.clone(),
                    Some(_) => vec![String::new()],
                    None => vec![ctext],
                };
                for child in children {
                    info.includes.push(Include {
                        parent: owner.clone(),
                        child,
                        prefix: prefix.clone(),
                        line,
                        mount: false,
                    });
                }
            }
            "mount" => {
                let path = positional(node, 0).or_else(|| kwarg(node, self.src, "path"));
                let app = positional(node, 1).or_else(|| kwarg(node, self.src, "app"));
                let (Some(path), Some(app)) = (path, app) else {
                    return;
                };
                info.includes.push(Include {
                    parent: owner,
                    child: text(app, self.src).to_string(),
                    prefix: eval_pstr(path, self.src),
                    line,
                    mount: true,
                });
            }
            "add_websocket_route" | "add_api_websocket_route" => {
                let path = positional(node, 0).or_else(|| kwarg(node, self.src, "path"));
                info.skipped.push(Skipped {
                    owner,
                    path: path
                        .map(|p| text(p, self.src).to_string())
                        .unwrap_or_default(),
                    line,
                });
            }
            "add_api_route" => {
                let path = positional(node, 0).or_else(|| kwarg(node, self.src, "path"));
                let ep = positional(node, 1).or_else(|| kwarg(node, self.src, "endpoint"));
                let methods =
                    methods_of(node, self.src).unwrap_or_else(|| Some(vec!["GET".to_string()]));
                let (Some(path), Some(methods)) = (path, methods) else {
                    return;
                };
                let mut handler = None;
                let mut params = vec![];
                if let Some(ep) = ep {
                    if ep.kind() == "identifier" {
                        let name = text(ep, self.src);
                        handler = Some(name.to_string());
                        if let Some(f) = self.funcs.get(name) {
                            params = classify_params(*f, self.src, self.enums);
                        }
                    }
                }
                info.routes.push(RouteDecl {
                    owner,
                    methods,
                    path: eval_pstr(path, self.src),
                    line,
                    handler,
                    params,
                });
            }
            _ => {}
        }
    }

    fn decorated(&mut self, node: Node<'t>, info: &mut FileInfo) {
        let Some(def) = node.child_by_field_name("definition") else {
            return;
        };
        if def.kind() != "function_definition" {
            return;
        }
        let handler = def
            .child_by_field_name("name")
            .map(|n| text(n, self.src).to_string());
        let mut params: Option<Vec<String>> = None;
        let mut c = node.walk();
        for dec in node.children(&mut c) {
            if dec.kind() != "decorator" {
                continue;
            }
            let Some(call) = dec.named_child(0) else {
                continue;
            };
            if call.kind() != "call" {
                continue;
            }
            let Some(func) = call.child_by_field_name("function") else {
                continue;
            };
            if func.kind() != "attribute" {
                continue;
            }
            let (Some(obj), Some(attr)) = (
                func.child_by_field_name("object"),
                func.child_by_field_name("attribute"),
            ) else {
                continue;
            };
            let attr = text(attr, self.src);
            if attr == "websocket" {
                let path = positional(call, 0).or_else(|| kwarg(call, self.src, "path"));
                let owner = self.qualify(text(obj, self.src), info, true);
                info.skipped.push(Skipped {
                    owner,
                    path: path
                        .map(|p| text(p, self.src).to_string())
                        .unwrap_or_default(),
                    line: dec.start_position().row as u32 + 1,
                });
                continue;
            }
            let methods = if HTTP_METHODS.contains(&attr) {
                Some(vec![attr.to_ascii_uppercase()])
            } else if attr == "api_route" {
                match methods_of(call, self.src) {
                    None => Some(vec!["GET".to_string()]),
                    Some(m) => m,
                }
            } else {
                continue;
            };
            let Some(methods) = methods else { continue };
            let path = positional(call, 0).or_else(|| kwarg(call, self.src, "path"));
            let Some(path) = path else { continue };
            let params = params
                .get_or_insert_with(|| classify_params(def, self.src, self.enums))
                .clone();
            let owner = self.qualify(text(obj, self.src), info, true);
            info.routes.push(RouteDecl {
                owner,
                methods,
                path: eval_pstr(path, self.src),
                line: dec.start_position().row as u32 + 1,
                handler: handler.clone(),
                params,
            });
        }
    }
}

/// `methods=[...]` of a call: outer `None` = argument absent, inner `None` = dynamic.
fn methods_of(call: Node, src: &str) -> Option<Option<Vec<String>>> {
    let v = kwarg(call, src, "methods")?;
    if !matches!(v.kind(), "list" | "tuple" | "set") {
        return Some(None);
    }
    let mut out = vec![];
    let mut c = v.walk();
    for e in v.named_children(&mut c) {
        match eval_pstr(e, src).and_then(lit_only) {
            Some(s) => out.push(s.to_ascii_uppercase()),
            None => return Some(None),
        }
    }
    if out.is_empty() {
        return Some(None);
    }
    Some(Some(out))
}

fn lit_only(p: PStr) -> Option<String> {
    let mut s = String::new();
    for part in p {
        match part {
            Part::Lit(l) => s.push_str(&l),
            Part::Ref(_) => return None,
        }
    }
    Some(s)
}

fn arg_list(call: Node) -> Option<Node> {
    call.child_by_field_name("arguments")
        .filter(|a| a.kind() == "argument_list")
}

fn positional(call: Node, idx: usize) -> Option<Node> {
    let args = arg_list(call)?;
    let mut c = args.walk();
    let found = args
        .named_children(&mut c)
        .filter(|a| {
            !matches!(
                a.kind(),
                "keyword_argument" | "comment" | "dictionary_splat" | "list_splat"
            )
        })
        .nth(idx);
    found
}

fn kwarg<'t>(call: Node<'t>, src: &str, name: &str) -> Option<Node<'t>> {
    let args = arg_list(call)?;
    let mut c = args.walk();
    let found = args
        .named_children(&mut c)
        .filter(|a| a.kind() == "keyword_argument")
        .find(|a| a.child_by_field_name("name").map(|k| text(k, src)) == Some(name))
        .and_then(|a| a.child_by_field_name("value"));
    found
}

// ---------------------------------------------------------------------------
// String evaluation
// ---------------------------------------------------------------------------

fn eval_pstr(n: Node, src: &str) -> Option<PStr> {
    eval_pstr_d(n, src, 0)
}

fn eval_pstr_d(n: Node, src: &str, depth: usize) -> Option<PStr> {
    if depth > 50 {
        return None;
    }
    match n.kind() {
        "string" => {
            let mut c = n.walk();
            let mut start = 0;
            let mut end = 0;
            for ch in n.children(&mut c) {
                match ch.kind() {
                    "interpolation" => return None,
                    "string_start" => start = ch.end_byte(),
                    "string_end" => end = ch.start_byte(),
                    _ => {}
                }
            }
            if end < start {
                return None;
            }
            Some(vec![Part::Lit(src.get(start..end)?.to_string())])
        }
        "concatenated_string" => {
            let mut out = vec![];
            let mut c = n.walk();
            for ch in n.named_children(&mut c) {
                if ch.kind() == "comment" {
                    continue;
                }
                out.extend(eval_pstr_d(ch, src, depth + 1)?);
            }
            Some(out)
        }
        "binary_operator" => {
            let op = n
                .child_by_field_name("operator")
                .map(|o| text(o, src))
                .unwrap_or("");
            if op != "+" {
                return None;
            }
            let mut l = eval_pstr_d(n.child_by_field_name("left")?, src, depth + 1)?;
            l.extend(eval_pstr_d(
                n.child_by_field_name("right")?,
                src,
                depth + 1,
            )?);
            Some(l)
        }
        "parenthesized_expression" => eval_pstr_d(n.named_child(0)?, src, depth + 1),
        "identifier" | "attribute" => {
            let t = text(n, src);
            if t.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            {
                Some(vec![Part::Ref(t.to_string())])
            } else {
                None
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Parameter classification
// ---------------------------------------------------------------------------

#[derive(PartialEq, Clone, Copy, Debug)]
enum Verdict {
    /// Explicit `Query(...)`.
    Query,
    /// Body / Depends / Header / ...
    Exclude,
    /// A simple (scalar-ish) type, `None` literal.
    Simple,
    /// Anything else (pydantic models, Request, Session, ...).
    Unknown,
}

fn classify_params(func: Node, src: &str, enums: &HashSet<String>) -> Vec<String> {
    let mut out = vec![];
    let Some(params) = func.child_by_field_name("parameters") else {
        return out;
    };
    let mut c = params.walk();
    for p in params.named_children(&mut c) {
        let (name, ann, default) = match p.kind() {
            "identifier" => (text(p, src).to_string(), None, None),
            "typed_parameter" => {
                let Some(id) = p.named_child(0).filter(|n| n.kind() == "identifier") else {
                    continue;
                };
                (
                    text(id, src).to_string(),
                    p.child_by_field_name("type"),
                    None,
                )
            }
            "default_parameter" => {
                let Some(n) = p
                    .child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")
                else {
                    continue;
                };
                (
                    text(n, src).to_string(),
                    None,
                    p.child_by_field_name("value"),
                )
            }
            "typed_default_parameter" => {
                let Some(n) = p
                    .child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")
                else {
                    continue;
                };
                (
                    text(n, src).to_string(),
                    p.child_by_field_name("type"),
                    p.child_by_field_name("value"),
                )
            }
            _ => continue,
        };
        if name == "self" || name == "cls" {
            continue;
        }
        if is_query_param(ann, default, src, enums) {
            out.push(name);
        }
    }
    out
}

fn is_query_param(
    ann: Option<Node>,
    default: Option<Node>,
    src: &str,
    enums: &HashSet<String>,
) -> bool {
    if let Some(d) = default {
        if d.kind() == "call" {
            let fname = d
                .child_by_field_name("function")
                .map(|f| tail_name(text(f, src)))
                .unwrap_or("");
            if fname == "Query" {
                return true;
            }
            if EXCLUDED_DEFAULTS.contains(&fname) {
                return false;
            }
        }
    }
    match ann {
        Some(a) => matches!(
            classify_ann(a, src, enums, 0),
            Verdict::Query | Verdict::Simple
        ),
        // Untyped: FastAPI treats it as a `str` query parameter unless the
        // default is some call / object we do not understand.
        None => match default {
            None => true,
            Some(d) => !matches!(d.kind(), "call" | "attribute" | "identifier" | "lambda"),
        },
    }
}

fn combine(vs: impl Iterator<Item = Verdict>) -> Verdict {
    let mut out = Verdict::Simple;
    for v in vs {
        match v {
            Verdict::Exclude => return Verdict::Exclude,
            Verdict::Unknown => out = Verdict::Unknown,
            Verdict::Query => {
                if out == Verdict::Simple {
                    out = Verdict::Query;
                }
            }
            Verdict::Simple => {}
        }
    }
    out
}

fn classify_ann(n: Node, src: &str, enums: &HashSet<String>, depth: usize) -> Verdict {
    if depth > 30 {
        return Verdict::Unknown;
    }
    let simple_name = |name: &str| -> Verdict {
        let t = tail_name(name);
        if SCALARS.contains(&t) || enums.contains(t) {
            Verdict::Simple
        } else {
            Verdict::Unknown
        }
    };
    match n.kind() {
        "type" | "parenthesized_expression" => match n.named_child(0) {
            Some(c) => classify_ann(c, src, enums, depth + 1),
            None => Verdict::Unknown,
        },
        "none" => Verdict::Simple,
        "identifier" | "attribute" => simple_name(text(n, src)),
        "string" => {
            let t = text(n, src).trim_matches(|c| c == '"' || c == '\'');
            simple_name(t)
        }
        "binary_operator" | "union_type" => {
            let mut c = n.walk();
            let vs: Vec<Verdict> = n
                .named_children(&mut c)
                .map(|x| classify_ann(x, src, enums, depth + 1))
                .collect();
            combine(vs.into_iter())
        }
        "generic_type" | "subscript" => {
            let base_node = n.child_by_field_name("value").or_else(|| n.named_child(0));
            let base = base_node.map(|b| tail_name(text(b, src))).unwrap_or("");
            let mut args: Vec<Node> = vec![];
            let mut c = n.walk();
            for ch in n.named_children(&mut c) {
                if Some(ch) == base_node {
                    continue;
                }
                if ch.kind() == "type_parameter" {
                    let mut c2 = ch.walk();
                    args.extend(ch.named_children(&mut c2));
                } else {
                    args.push(ch);
                }
            }
            if base == "Annotated" {
                let Some(first) = args.first() else {
                    return Verdict::Unknown;
                };
                for meta in args.iter().skip(1) {
                    let meta = if meta.kind() == "type" {
                        meta.named_child(0).unwrap_or(*meta)
                    } else {
                        *meta
                    };
                    if meta.kind() == "call" {
                        let f = meta
                            .child_by_field_name("function")
                            .map(|f| tail_name(text(f, src)))
                            .unwrap_or("");
                        if f == "Query" {
                            return Verdict::Query;
                        }
                        if EXCLUDED_DEFAULTS.contains(&f) {
                            return Verdict::Exclude;
                        }
                    }
                }
                return classify_ann(*first, src, enums, depth + 1);
            }
            if base == "Literal" {
                return Verdict::Simple;
            }
            if CONTAINERS.contains(&base) {
                if args.is_empty() {
                    return Verdict::Unknown;
                }
                let vs: Vec<Verdict> = args
                    .iter()
                    .map(|a| classify_ann(*a, src, enums, depth + 1))
                    .collect();
                return combine(vs.into_iter());
            }
            Verdict::Unknown
        }
        _ => Verdict::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Cross-module resolution and emission
// ---------------------------------------------------------------------------

enum Ev {
    /// (file index, index into `FileInfo::routes`)
    Route(usize, usize),
    /// `include_router` / `mount` of a child instance with its (static) prefix
    Child(Key, Option<String>),
}

type EventKey = (bool, usize, u32);

struct Resolver<'a> {
    infos: &'a [FileInfo],
    by_rel: HashMap<&'a str, usize>,
    /// dotted suffix -> candidate files
    by_suffix: HashMap<String, Vec<usize>>,
    /// module-level `app = create_app()` -> the instance the factory returns
    alias: HashMap<Key, Key>,
    /// instance -> registration events in runtime order
    events: HashMap<Key, Vec<(EventKey, Ev)>>,
    /// instances that were included / mounted somewhere
    has_parent: HashSet<Key>,
    /// include_router calls (outside tests) whose parent or child is not a known instance
    unresolved_includes: usize,
    /// (file, line, description) of mounts that are not followed
    skipped_mounts: Vec<(usize, u32, String)>,
    skipped_ws: Vec<(usize, u32, String)>,
}

struct Walk {
    visiting: Vec<Key>,
    visited: HashSet<Key>,
    seen: HashSet<(String, String, String, u32)>,
    out: Vec<(Route, bool)>,
}

fn is_test_file(rel: &str) -> bool {
    let mut it = rel.split('/').peekable();
    while let Some(c) = it.next() {
        if it.peek().is_some() {
            if matches!(c, "tests" | "test" | "testing") {
                return true;
            }
        } else {
            return c.starts_with("test_") || c.ends_with("_test.py") || c == "conftest.py";
        }
    }
    false
}

fn list_some(items: &[String], n: usize) -> String {
    let mut s = items.iter().take(n).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > n {
        s.push_str(&format!(", and {} more", items.len() - n));
    }
    s
}

impl<'a> Resolver<'a> {
    fn new(infos: &'a [FileInfo]) -> Self {
        let mut by_rel = HashMap::new();
        let mut by_suffix: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, f) in infos.iter().enumerate() {
            by_rel.insert(f.rel.as_str(), i);
            let mut comps: Vec<&str> = f.rel.split('/').collect();
            if let Some(last) = comps.pop() {
                let stem = last.strip_suffix(".py").unwrap_or(last);
                if stem != "__init__" {
                    comps.push(stem);
                }
            }
            for s in 0..comps.len() {
                by_suffix.entry(comps[s..].join(".")).or_default().push(i);
            }
        }
        let mut r = Resolver {
            infos,
            by_rel,
            by_suffix,
            alias: HashMap::new(),
            events: HashMap::new(),
            has_parent: HashSet::new(),
            unresolved_includes: 0,
            skipped_mounts: vec![],
            skipped_ws: vec![],
        };
        r.build_aliases();
        r.build_events();
        r
    }

    fn build_aliases(&mut self) {
        let mut alias = HashMap::new();
        for (fi, f) in self.infos.iter().enumerate() {
            for (name, callee) in &f.aliases {
                if f.instances.contains_key(name) {
                    continue;
                }
                if let Some((dfi, fname)) =
                    self.resolve_sym(fi, callee, &|f, n| f.returned_fns.contains_key(n))
                {
                    let target = self.infos[dfi].returned_fns[&fname].clone();
                    alias.insert((fi, name.clone()), (dfi, target));
                }
            }
        }
        self.alias = alias;
    }

    fn build_events(&mut self) {
        let mut events: HashMap<Key, Vec<(EventKey, Ev)>> = HashMap::new();
        let mut has_parent = HashSet::new();
        let (mut unresolved, mut mounts, mut ws) = (0usize, vec![], vec![]);
        for (fi, f) in self.infos.iter().enumerate() {
            for (ri, r) in f.routes.iter().enumerate() {
                if let Some(k) = self.resolve_inst(fi, &r.owner) {
                    events
                        .entry(k.clone())
                        .or_default()
                        .push(((fi != k.0, fi, r.line), Ev::Route(fi, ri)));
                }
            }
            for sk in &f.skipped {
                if self.resolve_inst(fi, &sk.owner).is_some() {
                    ws.push((fi, sk.line, sk.path.clone()));
                }
            }
            for inc in &f.includes {
                let parent = self.resolve_inst(fi, &inc.parent);
                let child = if inc.child.is_empty() {
                    None
                } else {
                    self.resolve_inst(fi, &inc.child)
                };
                let prefix = inc.prefix.as_ref().and_then(|p| self.eval_parts(fi, p, 0));
                if inc.mount {
                    let Some(parent) = parent else { continue };
                    match child {
                        Some(c) if c != parent && self.infos[c.0].instances[&c.1].is_app => {
                            has_parent.insert(c.clone());
                            events
                                .entry(parent.clone())
                                .or_default()
                                .push(((fi != parent.0, fi, inc.line), Ev::Child(c, prefix)));
                        }
                        _ => mounts.push((
                            fi,
                            inc.line,
                            prefix.unwrap_or_else(|| "(dynamic)".to_string()),
                        )),
                    }
                    continue;
                }
                match (parent, child) {
                    (Some(p), Some(c)) if p != c => {
                        has_parent.insert(c.clone());
                        events
                            .entry(p.clone())
                            .or_default()
                            .push(((fi != p.0, fi, inc.line), Ev::Child(c, prefix)));
                    }
                    (Some(_), Some(_)) => {}
                    _ => {
                        if !is_test_file(&f.rel) {
                            unresolved += 1;
                        }
                    }
                }
            }
        }
        // `setup(app)`-style functions: the call site is where their routes get registered.
        for (fi, f) in self.infos.iter().enumerate() {
            for call in &f.calls {
                let Some((dfi, fname)) =
                    self.resolve_sym(fi, &call.callee, &|f, n| f.fn_params.contains_key(n))
                else {
                    continue;
                };
                let params = &self.infos[dfi].fn_params[&fname];
                let named = call
                    .args
                    .iter()
                    .enumerate()
                    .filter_map(|(i, a)| params.get(i).map(|p| (p.as_str(), a.as_str())));
                let kw = call.kwargs.iter().map(|(k, v)| (k.as_str(), v.as_str()));
                for (param, arg) in named.chain(kw) {
                    if param.is_empty() || arg.is_empty() {
                        continue;
                    }
                    let vk = (dfi, format!("{}::{}", fname, param));
                    let Some(x) = self.resolve_inst(fi, arg) else {
                        continue;
                    };
                    if x == vk || !self.infos[dfi].instances.contains_key(&vk.1) {
                        continue;
                    }
                    has_parent.insert(vk.clone());
                    events.entry(x.clone()).or_default().push((
                        (fi != x.0, fi, call.line),
                        Ev::Child(vk, Some(String::new())),
                    ));
                }
            }
        }
        for (fi, f) in self.infos.iter().enumerate() {
            for (name, i) in &f.instances {
                if i.param_of.is_some() && !has_parent.contains(&(fi, name.clone())) {
                    unresolved += 1;
                }
            }
        }
        for v in events.values_mut() {
            v.sort_by_key(|(k, _)| *k);
        }
        self.events = events;
        self.has_parent = has_parent;
        self.unresolved_includes = unresolved;
        self.skipped_mounts = mounts;
        self.skipped_ws = ws;
    }

    fn resolve_module(&self, from: usize, level: usize, dotted: &str) -> Option<usize> {
        let from_rel = &self.infos[from].rel;
        let mut dir: Vec<&str> = from_rel.split('/').collect();
        dir.pop();
        if level > 0 {
            for _ in 1..level {
                dir.pop()?;
            }
            let mut base = dir.join("/");
            if !dotted.is_empty() {
                if !base.is_empty() {
                    base.push('/');
                }
                base.push_str(&dotted.replace('.', "/"));
            }
            let cands = if dotted.is_empty() && base.is_empty() {
                vec!["__init__.py".to_string()]
            } else if dotted.is_empty() {
                vec![format!("{}/__init__.py", base)]
            } else {
                vec![format!("{}.py", base), format!("{}/__init__.py", base)]
            };
            return cands
                .iter()
                .find_map(|c| self.by_rel.get(c.as_str()).copied());
        }
        if dotted.is_empty() {
            return None;
        }
        let cands = self.by_suffix.get(dotted)?;
        cands.iter().copied().max_by_key(|&i| {
            let other: Vec<&str> = self.infos[i].rel.split('/').collect();
            let common = other
                .iter()
                .zip(dir.iter())
                .take_while(|(a, b)| a == b)
                .count();
            (common, std::cmp::Reverse(other.len()))
        })
    }

    /// Find `name` defined (per `has`) in file `fi`, following `from x import name` re-exports.
    fn lookup_name(
        &self,
        fi: usize,
        name: &str,
        has: &dyn Fn(&FileInfo, &str) -> bool,
        depth: usize,
    ) -> Option<Key> {
        if depth > 12 {
            return None;
        }
        let f = &self.infos[fi];
        if has(f, name) {
            return Some((fi, name.to_string()));
        }
        let imp = f.imports.get(name)?;
        let sym = imp.sym.as_ref()?;
        let m = self.resolve_module(fi, imp.level, &imp.module)?;
        if m == fi {
            return None;
        }
        self.lookup_name(m, sym, has, depth + 1)
    }

    fn resolve_sym(
        &self,
        fi: usize,
        expr: &str,
        has: &dyn Fn(&FileInfo, &str) -> bool,
    ) -> Option<Key> {
        if expr.contains("::") {
            // function-local instance, qualified by the extractor
            return has(&self.infos[fi], expr).then(|| (fi, expr.to_string()));
        }
        let parts: Vec<&str> = expr.split('.').map(|s| s.trim()).collect();
        if parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_alphanumeric() || c == '_'))
        {
            return None;
        }
        if parts.len() == 1 {
            return self.lookup_name(fi, parts[0], has, 0);
        }
        let imp = self.infos[fi].imports.get(parts[0])?;
        let mut dotted = imp.module.clone();
        if let Some(s) = &imp.sym {
            if !dotted.is_empty() {
                dotted.push('.');
            }
            dotted.push_str(s);
        }
        for mid in &parts[1..parts.len() - 1] {
            if !dotted.is_empty() {
                dotted.push('.');
            }
            dotted.push_str(mid);
        }
        let m = self.resolve_module(fi, imp.level, &dotted)?;
        self.lookup_name(m, parts[parts.len() - 1], has, 0)
    }

    fn resolve_inst(&self, fi: usize, expr: &str) -> Option<Key> {
        let k = self.resolve_sym(fi, expr, &|f, n| {
            f.instances.contains_key(n) || f.aliases.contains_key(n)
        })?;
        match self.alias.get(&k) {
            Some(t) => Some(t.clone()),
            None => self.infos[k.0].instances.contains_key(&k.1).then_some(k),
        }
    }

    fn eval_parts(&self, fi: usize, parts: &PStr, depth: usize) -> Option<String> {
        if depth > 10 {
            return None;
        }
        let mut out = String::new();
        for p in parts {
            match p {
                Part::Lit(l) => out.push_str(l),
                Part::Ref(r) => {
                    let (cf, cn) = self.resolve_sym(fi, r, &|f, n| f.consts.contains_key(n))?;
                    let v = self.infos[cf].consts.get(&cn)?;
                    out.push_str(&self.eval_parts(cf, v, depth + 1)?);
                }
            }
        }
        Some(out)
    }

    fn inst(&self, k: &Key) -> &Inst {
        &self.infos[k.0].instances[&k.1]
    }

    fn own_prefix(&self, k: &Key) -> Option<String> {
        let p = self.inst(k).prefix.as_ref()?;
        self.eval_parts(k.0, p, 0)
    }

    /// Depth-first expansion in registration order. `acc` is everything above
    /// router `k` (parents' own prefixes + include prefixes), so the final
    /// path is the plain concatenation Starlette builds, trailing slash included.
    fn walk(&self, k: &Key, acc: &str, st: &mut Walk) {
        st.visited.insert(k.clone());
        if st.visiting.contains(k) {
            return;
        }
        let Some(own) = self.own_prefix(k) else {
            return;
        };
        let Some(events) = self.events.get(k) else {
            return;
        };
        st.visiting.push(k.clone());
        for (_, ev) in events {
            match ev {
                Ev::Child(child, prefix) => {
                    if let Some(p) = prefix {
                        self.walk(child, &format!("{}{}{}", acc, own, p), st);
                    } else {
                        st.visited.insert(child.clone());
                    }
                }
                Ev::Route(fi, ri) => {
                    let f = &self.infos[*fi];
                    let r = &f.routes[*ri];
                    let Some(mut path) = r.path.as_ref().and_then(|p| self.eval_parts(*fi, p, 0))
                    else {
                        continue;
                    };
                    if !path.is_empty() && !path.starts_with('/') {
                        path.insert(0, '/');
                    }
                    let raw = format!("{}{}{}", acc, own, path);
                    let full = naming::normalize_path_keep_slash(&raw);
                    let multi = raw.contains(":path}");
                    let pp = naming::path_params(&full);
                    let q: Vec<String> = r
                        .params
                        .iter()
                        .filter(|n| !pp.contains(n))
                        .cloned()
                        .collect();
                    for m in &r.methods {
                        if !st
                            .seen
                            .insert((m.clone(), full.clone(), f.rel.clone(), r.line))
                        {
                            continue;
                        }
                        st.out.push((
                            Route {
                                method: m.clone(),
                                path: full.clone(),
                                file: f.rel.clone(),
                                line: r.line,
                                handler: r.handler.clone(),
                                query_params: q.clone(),
                                framework: FRAMEWORK.to_string(),
                            },
                            multi,
                        ));
                    }
                }
            }
        }
        st.visiting.pop();
    }

    fn emit(&self) -> (Vec<Route>, Vec<String>) {
        let mut warnings: Vec<String> = vec![];
        let mut keys: Vec<Key> = vec![];
        for (fi, f) in self.infos.iter().enumerate() {
            for name in f.instances.keys() {
                keys.push((fi, name.clone()));
            }
        }
        keys.sort_by_key(|k| (k.0, self.inst(k).line, k.1.clone()));

        let app_roots: Vec<Key> = keys
            .iter()
            .filter(|k| {
                let i = self.inst(k);
                i.is_app
                    && !self.has_parent.contains(k)
                    && (!i.local || self.infos[k.0].returned.contains(&k.1))
            })
            .cloned()
            .collect();
        let orphans: Vec<Key> = keys
            .iter()
            .filter(|k| !self.inst(k).is_app && !self.has_parent.contains(k))
            .cloned()
            .collect();
        let roots: Vec<Key> = if app_roots.is_empty() {
            warnings.push(
                "no module-level FastAPI() app (or app factory) found; emitting the routes of every router, \
                 so routes that are never included in an app may be listed"
                    .to_string(),
            );
            keys.iter()
                .filter(|k| !self.has_parent.contains(k))
                .cloned()
                .collect()
        } else if self.unresolved_includes > 0 && !orphans.is_empty() {
            warnings.push(format!(
                "{} include_router() call(s) could not be resolved statically; \
                 routers that are not included anywhere are still emitted",
                self.unresolved_includes
            ));
            let mut r = app_roots.clone();
            r.extend(orphans.iter().cloned());
            r
        } else {
            app_roots.clone()
        };

        let mut st = Walk {
            visiting: vec![],
            visited: HashSet::new(),
            seen: HashSet::new(),
            out: vec![],
        };
        for r in &roots {
            self.walk(r, "", &mut st);
        }

        // Routers / apps that no app reaches.
        let mut dead: HashMap<Key, usize> = HashMap::new();
        for (fi, f) in self.infos.iter().enumerate() {
            for r in &f.routes {
                if let Some(k) = self.resolve_inst(fi, &r.owner) {
                    if !st.visited.contains(&k) {
                        *dead.entry(k).or_default() += r.methods.len();
                    }
                }
            }
        }
        if !dead.is_empty() {
            let mut d: Vec<(Key, usize)> = dead.into_iter().collect();
            d.sort_by_key(|(k, _)| (k.0, self.inst(k).line));
            let n: usize = d.iter().map(|(_, n)| n).sum();
            let items: Vec<String> = d
                .iter()
                .map(|(k, n)| {
                    format!(
                        "{} `{}` ({}:{}, {} route(s))",
                        if self.inst(k).is_app { "app" } else { "router" },
                        k.1.rsplit("::").next().unwrap_or(&k.1),
                        self.infos[k.0].rel,
                        self.inst(k).line,
                        n
                    )
                })
                .collect();
            warnings.push(format!(
                "skipped {} route(s) on {} router/app object(s) that no module-level FastAPI app includes \
                 (never passed to include_router / mount, or created inside a function): {}",
                n,
                d.len(),
                list_some(&items, 6)
            ));
        }

        // Duplicate method + path: FastAPI serves the first registered route.
        let mut first: HashMap<(String, String), usize> = HashMap::new();
        for (i, (r, _)) in st.out.iter().enumerate() {
            match first.get(&(r.method.clone(), r.path.clone())) {
                None => {
                    first.insert((r.method.clone(), r.path.clone()), i);
                }
                Some(&j) => {
                    let w = &st.out[j].0;
                    warnings.push(format!(
                        "duplicate route {} {}: {}:{} is shadowed by {}:{} (FastAPI serves the first registered route)",
                        r.method, r.path, r.file, r.line, w.file, w.line
                    ));
                }
            }
        }

        let multi: Vec<String> = st
            .out
            .iter()
            .filter(|(_, m)| *m)
            .map(|(r, _)| format!("{} {} ({}:{})", r.method, r.path, r.file, r.line))
            .collect();
        if !multi.is_empty() {
            warnings.push(format!(
                "{} route(s) use a `{{name:path}}` converter, which matches several path segments (the parameter is written as `{{name}}`): {}",
                multi.len(),
                list_some(&multi, 8)
            ));
        }

        let loc = |(fi, line, p): &(usize, u32, String)| {
            format!("{} ({}:{})", p, self.infos[*fi].rel, line)
        };
        if !self.skipped_ws.is_empty() || !self.skipped_mounts.is_empty() {
            let mut parts = vec![];
            if !self.skipped_ws.is_empty() {
                let v: Vec<String> = self.skipped_ws.iter().map(loc).collect();
                parts.push(format!(
                    "{} websocket route(s): {}",
                    v.len(),
                    list_some(&v, 6)
                ));
            }
            if !self.skipped_mounts.is_empty() {
                let v: Vec<String> = self.skipped_mounts.iter().map(loc).collect();
                parts.push(format!("{} mount(s): {}", v.len(), list_some(&v, 6)));
            }
            warnings.push(format!("not scanned: {}", parts.join("; ")));
        }

        (st.out.into_iter().map(|(r, _)| r).collect(), warnings)
    }
}
