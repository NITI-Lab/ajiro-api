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
}

#[derive(Default)]
struct FileInfo {
    rel: String,
    imports: HashMap<String, Import>,
    /// router instances; value = own prefix (`None` = dynamic)
    instances: HashMap<String, Option<PStr>>,
    consts: HashMap<String, PStr>,
    routes: Vec<RouteDecl>,
    includes: Vec<Include>,
}

struct Parsed {
    rel: String,
    src: String,
    tree: tree_sitter::Tree,
}

type Key = (usize, String);

pub fn scan(root: &Path, files: &[PathBuf]) -> Result<Vec<Route>> {
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
}

impl<'a, 't> Extract<'a, 't> {
    fn walk(&mut self, node: Node<'t>, info: &mut FileInfo, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match node.kind() {
            "decorated_definition" => self.decorated(node, info),
            "assignment" => self.assignment(node, info),
            "call" => self.call(node, info),
            _ => {}
        }
        let mut c = node.walk();
        for ch in node.children(&mut c) {
            self.walk(ch, info, depth + 1);
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
        info.instances.insert(text(l, self.src).to_string(), prefix);
    }

    fn call(&mut self, node: Node<'t>, info: &mut FileInfo) {
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
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
        let owner = text(obj, self.src).to_string();
        match attr {
            "include_router" => {
                let child = positional(node, 0).or_else(|| kwarg(node, self.src, "router"));
                let Some(child) = child else { return };
                let prefix = match kwarg(node, self.src, "prefix") {
                    None => Some(vec![]),
                    Some(v) => eval_pstr(v, self.src),
                };
                info.includes.push(Include {
                    parent: owner,
                    child: text(child, self.src).to_string(),
                    prefix,
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
                    line: node.start_position().row as u32 + 1,
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
            info.routes.push(RouteDecl {
                owner: text(obj, self.src).to_string(),
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

struct Resolver<'a> {
    infos: &'a [FileInfo],
    by_rel: HashMap<&'a str, usize>,
    /// dotted suffix -> candidate files
    by_suffix: HashMap<String, Vec<usize>>,
    /// child instance -> [(parent instance, resolved include prefix)]
    parents: HashMap<Key, Vec<(Key, Option<String>)>>,
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
            parents: HashMap::new(),
        };
        r.build_edges();
        r
    }

    fn build_edges(&mut self) {
        let mut edges: Vec<(Key, Key, Option<String>)> = vec![];
        for (fi, f) in self.infos.iter().enumerate() {
            for inc in &f.includes {
                let (Some(parent), Some(child)) = (
                    self.resolve_inst(fi, &inc.parent),
                    self.resolve_inst(fi, &inc.child),
                ) else {
                    continue;
                };
                if parent == child {
                    continue;
                }
                let prefix = inc.prefix.as_ref().and_then(|p| self.eval_parts(fi, p, 0));
                edges.push((child, parent, prefix));
            }
        }
        for (child, parent, prefix) in edges {
            let v = self.parents.entry(child).or_default();
            if !v.iter().any(|(p, x)| *p == parent && *x == prefix) {
                v.push((parent, prefix));
            }
        }
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
        self.resolve_sym(fi, expr, &|f, n| f.instances.contains_key(n))
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

    fn own_prefix(&self, k: &Key) -> Option<String> {
        let p = self.infos[k.0].instances.get(&k.1)?.as_ref()?;
        self.eval_parts(k.0, p, 0)
    }

    /// Prefixes accumulated above (excluding) router `k`.
    fn base(&self, k: &Key, visiting: &mut Vec<Key>) -> Vec<String> {
        let Some(parents) = self.parents.get(k) else {
            return vec![String::new()];
        };
        if visiting.contains(k) {
            return vec![];
        }
        visiting.push(k.clone());
        let mut out = vec![];
        for (p, inc) in parents {
            let (Some(inc), Some(own)) = (inc, self.own_prefix(p)) else {
                continue;
            };
            for b in self.base(p, visiting) {
                out.push(format!("{}{}{}", b, own, inc));
            }
        }
        visiting.pop();
        out
    }

    fn emit(&self) -> Vec<Route> {
        let mut out: Vec<Route> = vec![];
        let mut seen: HashSet<(String, String, String, u32)> = HashSet::new();
        for (fi, f) in self.infos.iter().enumerate() {
            for r in &f.routes {
                let Some(k) = self.resolve_inst(fi, &r.owner) else {
                    continue;
                };
                let Some(own) = self.own_prefix(&k) else {
                    continue;
                };
                let Some(path) = r.path.as_ref().and_then(|p| self.eval_parts(fi, p, 0)) else {
                    continue;
                };
                for b in self.base(&k, &mut vec![]) {
                    let full = naming::normalize_path(&format!("{}{}/{}", b, own, path));
                    let pp = naming::path_params(&full);
                    let q: Vec<String> = r
                        .params
                        .iter()
                        .filter(|n| !pp.contains(n))
                        .cloned()
                        .collect();
                    for m in &r.methods {
                        if !seen.insert((m.clone(), full.clone(), f.rel.clone(), r.line)) {
                            continue;
                        }
                        out.push(Route {
                            method: m.clone(),
                            path: full.clone(),
                            file: f.rel.clone(),
                            line: r.line,
                            handler: r.handler.clone(),
                            query_params: q.clone(),
                            framework: FRAMEWORK.to_string(),
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            (&a.file, a.line, &a.path, &a.method).cmp(&(&b.file, b.line, &b.path, &b.method))
        });
        out
    }
}
