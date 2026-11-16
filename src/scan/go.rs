//! Go route scanner: net/http (`ServeMux`, Go 1.22 patterns), chi and Echo.
//!
//! Every function / method body is evaluated symbolically into a small graph:
//! *routers* (created by `chi.NewRouter()`, `echo.New()`, `http.NewServeMux()`,
//! function parameters typed as routers, closures passed to `Route`/`Group`,
//! `e.Group("/api")`, ...), *edges* (child router mounted below a parent with a
//! prefix) and *routes* registered on a router.  Calls to functions of the
//! scanned code base are inlined (so `registerUsers(api)` and
//! `r.Mount("/admin", adminRouter())` compose their prefixes).  Functions that
//! are never called from another scanned function are the roots whose graphs
//! are flattened into [`Route`]s.
//!
//! Notes / limitations:
//! - `Any` (Echo) and method-less `Handle`/`HandleFunc` patterns are reported
//!   as `GET` only.
//! - Routers stored in struct fields or package variables are tracked per
//!   function by their source text (`s.router`), without cross-function flow.
//! - Dynamic (non-literal, non-constant) paths are skipped.

use super::Route;
use crate::naming;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use tree_sitter::{Node, Parser, Tree};

const MAX_DEPTH: usize = 300;
/// Marker for a dynamic (unknown) path fragment; routes containing it are dropped.
const DYN: &str = "\u{0}";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fw {
    NetHttp,
    Chi,
    Echo,
}

impl Fw {
    fn name(self) -> &'static str {
        match self {
            Fw::NetHttp => "go-nethttp",
            Fw::Chi => "go-chi",
            Fw::Echo => "go-echo",
        }
    }
}

const CHI_METHODS: &[&str] = &[
    "Get", "Post", "Put", "Patch", "Delete", "Head", "Options", "Connect", "Trace",
];
const ECHO_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];
const ROUTER_TYPES: &[&str] = &[
    "chi.Router",
    "chi.Mux",
    "echo.Echo",
    "echo.Group",
    "http.ServeMux",
    "ServeMux",
];
const HTTP_METHOD_TOKENS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];

struct GoFile {
    rel: String,
    dir: String,
    pkg: String,
    src: String,
    tree: Tree,
    fw: Fw,
}

type FnKey = (usize, usize);

struct FnLoc {
    key: FnKey,
    pkg: String,
    dir: String,
    is_method: bool,
}

#[derive(Clone)]
struct GRoute {
    router: usize,
    path: String,
    method: String,
    file: usize,
    line: u32,
    handler: Option<String>,
    fw: Fw,
}

#[derive(Clone, Default)]
struct Graph {
    n: usize,
    /// (parent, prefix, child)
    edges: Vec<(usize, String, usize)>,
    routes: Vec<GRoute>,
    /// (argument index, router id)
    params: Vec<(usize, usize)>,
    ret: Option<usize>,
    called: Vec<FnKey>,
}

pub fn scan(root: &Path, files: &[PathBuf]) -> Result<Vec<Route>> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_go::LANGUAGE.into())?;

    let mut parsed: Vec<GoFile> = Vec::new();
    for f in files {
        if f.extension().and_then(|e| e.to_str()) != Some("go") {
            continue;
        }
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with("_test.go") {
            continue;
        }
        let rel = rel_path(root, f);
        if rel.split('/').any(|c| c == "vendor") {
            continue;
        }
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let src = String::from_utf8_lossy(&bytes).into_owned();
        let Some(tree) = parser.parse(&src, None) else {
            continue;
        };
        let (pkg, fw) = file_meta(tree.root_node(), &src);
        let dir = match rel.rfind('/') {
            Some(i) => rel[..i].to_string(),
            None => String::new(),
        };
        parsed.push(GoFile {
            rel,
            dir,
            pkg,
            src,
            tree,
            fw,
        });
    }
    parsed.sort_by(|a, b| a.rel.cmp(&b.rel));

    // Index functions / methods and package level string constants.
    let mut fns: HashMap<String, Vec<FnLoc>> = HashMap::new();
    let mut consts: HashMap<(String, String), String> = HashMap::new();
    for (fi, f) in parsed.iter().enumerate() {
        let root_node = f.tree.root_node();
        let mut c = root_node.walk();
        for (ci, ch) in root_node.children(&mut c).enumerate() {
            match ch.kind() {
                "function_declaration" | "method_declaration" => {
                    if let Some(n) = ch.child_by_field_name("name") {
                        fns.entry(text(n, &f.src).to_string())
                            .or_default()
                            .push(FnLoc {
                                key: (fi, ci),
                                pkg: f.pkg.clone(),
                                dir: f.dir.clone(),
                                is_method: ch.kind() == "method_declaration",
                            });
                    }
                }
                "const_declaration" | "var_declaration" => collect_consts(ch, f, &mut consts, 0),
                _ => {}
            }
        }
    }

    let mut cx = Cx {
        files: &parsed,
        fns: &fns,
        consts: &consts,
        memo: HashMap::new(),
        stack: vec![],
    };
    let mut all_keys: Vec<FnKey> = fns.values().flatten().map(|l| l.key).collect();
    all_keys.sort();
    let mut graphs: Vec<(FnKey, Rc<Graph>)> = vec![];
    for k in &all_keys {
        let g = cx.summary(*k);
        graphs.push((*k, g));
    }
    let inlined: HashSet<FnKey> = graphs
        .iter()
        .flat_map(|(_, g)| g.called.iter().copied())
        .collect();

    let mut out: Vec<Route> = vec![];
    let mut seen: HashSet<(String, String, usize, u32)> = HashSet::new();
    for (k, g) in &graphs {
        if inlined.contains(k) {
            continue;
        }
        flatten(g, &parsed, &mut out, &mut seen);
    }
    out.sort_by(|a, b| {
        (&a.file, a.line, &a.path, &a.method).cmp(&(&b.file, b.line, &b.path, &b.method))
    });
    Ok(out)
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

fn file_meta(root: Node, src: &str) -> (String, Fw) {
    let mut pkg = String::new();
    let mut fw = Fw::NetHttp;
    let mut chi = false;
    let mut echo = false;
    let mut c = root.walk();
    for ch in root.children(&mut c) {
        match ch.kind() {
            "package_clause" => {
                let mut c2 = ch.walk();
                for x in ch.named_children(&mut c2) {
                    pkg = text(x, src).to_string();
                }
            }
            "import_declaration" => {
                let mut stack = vec![ch];
                while let Some(n) = stack.pop() {
                    if n.kind() == "import_spec" {
                        if let Some(p) = n.child_by_field_name("path") {
                            let t = text(p, src).trim_matches(|c| c == '"' || c == '`');
                            if t.starts_with("github.com/go-chi/chi") {
                                chi = true;
                            } else if t.starts_with("github.com/labstack/echo") {
                                echo = true;
                            }
                        }
                    } else {
                        let mut c3 = n.walk();
                        stack.extend(n.named_children(&mut c3));
                    }
                }
            }
            _ => {}
        }
    }
    if chi {
        fw = Fw::Chi;
    } else if echo {
        fw = Fw::Echo;
    }
    (pkg, fw)
}

fn collect_consts(
    decl: Node,
    f: &GoFile,
    out: &mut HashMap<(String, String), String>,
    depth: usize,
) {
    if depth > 6 {
        return;
    }
    let mut c = decl.walk();
    for ch in decl.named_children(&mut c) {
        match ch.kind() {
            "const_spec" | "var_spec" => {
                let mut c2 = ch.walk();
                let names: Vec<Node> = ch.children_by_field_name("name", &mut c2).collect();
                let Some(val) = ch.child_by_field_name("value") else {
                    continue;
                };
                let mut c3 = val.walk();
                let vals: Vec<Node> = val.named_children(&mut c3).collect();
                for (n, v) in names.iter().zip(vals.iter()) {
                    if let Some(s) = lit_string(*v, &f.src) {
                        out.insert((f.dir.clone(), text(*n, &f.src).to_string()), s);
                    }
                }
            }
            "const_spec_list" | "var_spec_list" => collect_consts(ch, f, out, depth + 1),
            _ => {}
        }
    }
}

/// Pure string literals and `+` concatenations of them.
fn lit_string(n: Node, src: &str) -> Option<String> {
    match n.kind() {
        "interpreted_string_literal" => {
            let t = text(n, src);
            let inner = t.get(1..t.len().checked_sub(1)?)?;
            Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
        }
        "raw_string_literal" => {
            let t = text(n, src);
            Some(t.get(1..t.len().checked_sub(1)?)?.to_string())
        }
        "binary_expression" => {
            let op = n.child_by_field_name("operator").map(|o| text(o, src))?;
            if op != "+" {
                return None;
            }
            let l = lit_string(n.child_by_field_name("left")?, src)?;
            let r = lit_string(n.child_by_field_name("right")?, src)?;
            Some(l + &r)
        }
        "parenthesized_expression" => lit_string(n.named_child(0)?, src),
        _ => None,
    }
}

fn is_string_lit(n: Node) -> bool {
    matches!(
        n.kind(),
        "interpreted_string_literal" | "raw_string_literal"
    )
}

// ---------------------------------------------------------------------------
// Flattening
// ---------------------------------------------------------------------------

fn flatten(
    g: &Graph,
    files: &[GoFile],
    out: &mut Vec<Route>,
    seen: &mut HashSet<(String, String, usize, u32)>,
) {
    let mut incoming: Vec<Vec<(usize, &str)>> = vec![vec![]; g.n];
    for (p, pre, c) in &g.edges {
        if *p < g.n && *c < g.n {
            incoming[*c].push((*p, pre.as_str()));
        }
    }
    let mut memo: HashMap<usize, Vec<String>> = HashMap::new();
    for r in &g.routes {
        let prefixes = prefixes_of(r.router, &incoming, &mut memo, &mut vec![]);
        for p in prefixes {
            let raw = format!("{}/{}", p, r.path);
            if raw.contains(DYN) {
                continue;
            }
            let path = naming::normalize_path(&raw);
            if !seen.insert((r.method.clone(), path.clone(), r.file, r.line)) {
                continue;
            }
            out.push(Route {
                method: r.method.clone(),
                path,
                file: files[r.file].rel.clone(),
                line: r.line,
                handler: r.handler.clone(),
                query_params: vec![],
                framework: r.fw.name().to_string(),
            });
        }
    }
}

fn prefixes_of(
    r: usize,
    incoming: &[Vec<(usize, &str)>],
    memo: &mut HashMap<usize, Vec<String>>,
    visiting: &mut Vec<usize>,
) -> Vec<String> {
    if let Some(v) = memo.get(&r) {
        return v.clone();
    }
    if incoming[r].is_empty() {
        return vec![String::new()];
    }
    if visiting.contains(&r) || visiting.len() > 64 {
        return vec![];
    }
    visiting.push(r);
    let mut out = vec![];
    for (p, pre) in &incoming[r] {
        for base in prefixes_of(*p, incoming, memo, visiting) {
            let v = format!("{}/{}", base, pre);
            if !out.contains(&v) {
                out.push(v);
            }
        }
    }
    visiting.pop();
    memo.insert(r, out.clone());
    out
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

struct Cx<'a> {
    files: &'a [GoFile],
    fns: &'a HashMap<String, Vec<FnLoc>>,
    consts: &'a HashMap<(String, String), String>,
    memo: HashMap<FnKey, Rc<Graph>>,
    stack: Vec<FnKey>,
}

impl<'a> Cx<'a> {
    fn summary(&mut self, key: FnKey) -> Rc<Graph> {
        if let Some(g) = self.memo.get(&key) {
            return g.clone();
        }
        if self.stack.contains(&key) || self.stack.len() > 40 {
            return Rc::new(Graph::default());
        }
        self.stack.push(key);
        let files = self.files;
        let f = &files[key.0];
        let node = f.tree.root_node().child(key.1 as u32);
        let mut ev = Ev {
            cx: self,
            g: Graph::default(),
            file: key.0,
            env: HashMap::new(),
        };
        if let Some(node) = node {
            ev.bind_params(node);
            if let Some(body) = node.child_by_field_name("body") {
                ev.walk(body, 0);
            }
        }
        let g = Rc::new(ev.g);
        self.stack.pop();
        self.memo.insert(key, g.clone());
        g
    }

    fn find_fn(&self, from_dir: &str, qual: Option<&str>, name: &str) -> Option<FnKey> {
        let c = self.fns.get(name)?;
        match qual {
            Some(q) => {
                let base = |d: &str| d.rsplit('/').next().unwrap_or("").to_string();
                c.iter()
                    .find(|f| !f.is_method && (f.pkg == q || base(&f.dir) == q))
                    .or_else(|| c.iter().find(|f| f.is_method && f.dir == from_dir))
                    .map(|f| f.key)
            }
            None => c
                .iter()
                .find(|f| !f.is_method && f.dir == from_dir)
                .map(|f| f.key),
        }
    }
}

struct Ev<'c, 'a> {
    cx: &'c mut Cx<'a>,
    g: Graph,
    file: usize,
    env: HashMap<String, usize>,
}

impl<'c, 'a> Ev<'c, 'a> {
    fn f(&self) -> &'a GoFile {
        &self.cx.files[self.file]
    }

    fn src(&self) -> &'a str {
        self.cx.files[self.file].src.as_str()
    }

    fn fw(&self) -> Fw {
        self.f().fw
    }

    fn new_router(&mut self) -> usize {
        self.g.n += 1;
        self.g.n - 1
    }

    fn child(&mut self, parent: usize, prefix: String) -> usize {
        let c = self.new_router();
        self.g.edges.push((parent, prefix, c));
        c
    }

    fn bind_params(&mut self, func: Node) {
        let src = self.src();
        let Some(params) = func.child_by_field_name("parameters") else {
            return;
        };
        let mut idx = 0usize;
        let mut c = params.walk();
        for p in params.named_children(&mut c) {
            match p.kind() {
                "parameter_declaration" | "variadic_parameter_declaration" => {
                    let ty = p
                        .child_by_field_name("type")
                        .map(|t| text(t, src))
                        .unwrap_or("");
                    let is_router = ROUTER_TYPES.iter().any(|t| ty.contains(t));
                    let mut c2 = p.walk();
                    let names: Vec<Node> = p.children_by_field_name("name", &mut c2).collect();
                    if names.is_empty() {
                        idx += 1;
                        continue;
                    }
                    for n in names {
                        if is_router {
                            let id = self.new_router();
                            self.env.insert(text(n, src).to_string(), id);
                            self.g.params.push((idx, id));
                        }
                        idx += 1;
                    }
                }
                _ => {}
            }
        }
    }

    fn go_string(&self, n: Node) -> Option<String> {
        let src = self.src();
        if let Some(s) = lit_string(n, src) {
            return Some(s);
        }
        match n.kind() {
            "identifier" => self
                .cx
                .consts
                .get(&(self.f().dir.clone(), text(n, src).to_string()))
                .cloned(),
            "parenthesized_expression" => self.go_string(n.named_child(0)?),
            "binary_expression" => {
                let op = n.child_by_field_name("operator").map(|o| text(o, src))?;
                if op != "+" {
                    return None;
                }
                let l = self.go_string(n.child_by_field_name("left")?)?;
                let r = self.go_string(n.child_by_field_name("right")?)?;
                Some(l + &r)
            }
            _ => None,
        }
    }

    fn path_or_dyn(&self, n: Option<Node>) -> String {
        n.and_then(|n| self.go_string(n))
            .unwrap_or_else(|| DYN.to_string())
    }

    // -- statement walking ---------------------------------------------------

    fn walk(&mut self, node: Node, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match node.kind() {
            "call_expression" => {
                self.eval_call(node, depth + 1);
            }
            "short_var_declaration" | "assignment_statement" => {
                let (Some(l), Some(r)) = (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) else {
                    return self.walk_children(node, depth);
                };
                self.bind_pairs(l, r, depth);
            }
            "var_spec" => {
                let (Some(_), Some(r)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("value"),
                ) else {
                    return self.walk_children(node, depth);
                };
                let mut c = node.walk();
                let names: Vec<Node> = node.children_by_field_name("name", &mut c).collect();
                let mut c2 = r.walk();
                let vals: Vec<Node> = r.named_children(&mut c2).collect();
                for (i, v) in vals.iter().enumerate() {
                    let id = self.eval_expr(*v, depth + 1);
                    if let (Some(id), Some(n)) = (id, names.get(i)) {
                        self.env.insert(text(*n, self.src()).to_string(), id);
                    }
                }
            }
            "return_statement" => {
                let mut c = node.walk();
                let exprs: Vec<Node> = node.named_children(&mut c).collect();
                for e in exprs {
                    let mut inner = vec![e];
                    if e.kind() == "expression_list" {
                        let mut c2 = e.walk();
                        inner = e.named_children(&mut c2).collect();
                    }
                    for (i, x) in inner.into_iter().enumerate() {
                        let id = self.eval_expr(x, depth + 1);
                        if i == 0 && self.g.ret.is_none() {
                            self.g.ret = id;
                        }
                    }
                }
            }
            "func_literal" => self.walk_closure(node, None, depth),
            _ => self.walk_children(node, depth),
        }
    }

    fn walk_children(&mut self, node: Node, depth: usize) {
        let mut c = node.walk();
        for ch in node.children(&mut c) {
            self.walk(ch, depth + 1);
        }
    }

    fn bind_pairs(&mut self, l: Node, r: Node, depth: usize) {
        let mut c = l.walk();
        let lhs: Vec<Node> = if l.kind() == "expression_list" {
            l.named_children(&mut c).collect()
        } else {
            vec![l]
        };
        let mut c2 = r.walk();
        let rhs: Vec<Node> = if r.kind() == "expression_list" {
            r.named_children(&mut c2).collect()
        } else {
            vec![r]
        };
        for (i, v) in rhs.iter().enumerate() {
            let id = self.eval_expr(*v, depth + 1);
            if let (Some(id), Some(n)) = (id, lhs.get(i)) {
                self.env.insert(text(*n, self.src()).to_string(), id);
            }
        }
    }

    /// Evaluate a func literal body; `bind` is the router for its first parameter.
    fn walk_closure(&mut self, lit: Node, bind: Option<usize>, depth: usize) {
        let saved = self.env.clone();
        if let (Some(id), Some(params)) = (bind, lit.child_by_field_name("parameters")) {
            let mut c = params.walk();
            let first = params
                .named_children(&mut c)
                .find(|p| p.kind() == "parameter_declaration")
                .and_then(|p| p.child_by_field_name("name"));
            if let Some(n) = first {
                self.env.insert(text(n, self.src()).to_string(), id);
            }
        }
        if let Some(body) = lit.child_by_field_name("body") {
            self.walk(body, depth + 1);
        }
        self.env = saved;
    }

    fn eval_expr(&mut self, n: Node, depth: usize) -> Option<usize> {
        if depth > MAX_DEPTH {
            return None;
        }
        match n.kind() {
            "call_expression" => self.eval_call(n, depth + 1),
            "identifier" | "selector_expression" => self.env.get(text(n, self.src())).copied(),
            "parenthesized_expression" => self.eval_expr(n.named_child(0)?, depth + 1),
            "unary_expression" => self.eval_expr(n.child_by_field_name("operand")?, depth + 1),
            "composite_literal" => {
                let t = n
                    .child_by_field_name("type")
                    .map(|t| text(t, self.src()))
                    .unwrap_or("");
                if t == "http.ServeMux" || t == "chi.Mux" {
                    Some(self.new_router())
                } else {
                    self.walk_children(n, depth);
                    None
                }
            }
            _ => {
                self.walk(n, depth + 1);
                None
            }
        }
    }

    // -- calls -----------------------------------------------------------------

    fn handler_name(&self, n: Node) -> Option<String> {
        let src = self.src();
        match n.kind() {
            "identifier" | "selector_expression" => Some(text(n, src).to_string()),
            "call_expression" => {
                let f = n.child_by_field_name("function")?;
                if text(f, src) == "http.HandlerFunc" {
                    let args = n.child_by_field_name("arguments")?;
                    return self.handler_name(args.named_child(0)?);
                }
                None
            }
            "parenthesized_expression" => self.handler_name(n.named_child(0)?),
            _ => None,
        }
    }

    fn add_route(
        &mut self,
        router: usize,
        method: &str,
        path: String,
        call: Node,
        handler: Option<Node>,
    ) {
        let handler = handler.and_then(|h| self.handler_name(h));
        self.g.routes.push(GRoute {
            router,
            path,
            method: method.to_ascii_uppercase(),
            file: self.file,
            line: call.start_position().row as u32 + 1,
            handler,
            fw: self.fw(),
        });
    }

    fn inline(&mut self, key: FnKey, args: &[Option<usize>]) -> Option<usize> {
        let callee = self.cx.summary(key);
        let off = self.g.n;
        self.g.n += callee.n;
        for (p, pre, c) in &callee.edges {
            self.g.edges.push((p + off, pre.clone(), c + off));
        }
        for r in &callee.routes {
            let mut r = r.clone();
            r.router += off;
            self.g.routes.push(r);
        }
        for (i, rid) in &callee.params {
            if let Some(Some(a)) = args.get(*i) {
                self.g.edges.push((*a, String::new(), rid + off));
            }
        }
        self.g.called.push(key);
        self.g.called.extend(callee.called.iter().copied());
        callee.ret.map(|r| r + off)
    }

    fn call_args<'t>(call: Node<'t>) -> Vec<Node<'t>> {
        let Some(a) = call.child_by_field_name("arguments") else {
            return vec![];
        };
        let mut c = a.walk();
        a.named_children(&mut c)
            .filter(|n| n.kind() != "comment")
            .collect()
    }

    fn eval_call(&mut self, call: Node, depth: usize) -> Option<usize> {
        if depth > MAX_DEPTH {
            return None;
        }
        let src = self.src();
        let args = Self::call_args(call);
        let func = call.child_by_field_name("function")?;
        match func.kind() {
            "selector_expression" => {
                let (Some(operand), Some(field)) = (
                    func.child_by_field_name("operand"),
                    func.child_by_field_name("field"),
                ) else {
                    return None;
                };
                let f = text(field, src);
                let op_text = text(operand, src);
                if operand.kind() == "identifier" {
                    match (op_text, f) {
                        ("chi", "NewRouter")
                        | ("chi", "NewMux")
                        | ("echo", "New")
                        | ("http", "NewServeMux") => {
                            self.walk_args(&args, depth);
                            return Some(self.new_router());
                        }
                        _ => {}
                    }
                }
                let recv = match operand.kind() {
                    "call_expression" => self.eval_call(operand, depth + 1),
                    "identifier" | "selector_expression" => self.env.get(op_text).copied(),
                    "parenthesized_expression" => operand
                        .named_child(0)
                        .and_then(|x| self.eval_expr(x, depth + 1)),
                    _ => None,
                };
                let recv = match recv {
                    Some(r) => Some(r),
                    None if matches!(operand.kind(), "identifier" | "selector_expression")
                        && self.auto_root_ok(op_text, f, &args) =>
                    {
                        let id = self.new_router();
                        self.env.insert(op_text.to_string(), id);
                        Some(id)
                    }
                    None => None,
                };
                if let Some(rid) = recv {
                    if let Some(res) = self.router_call(rid, f, &args, call, depth) {
                        return res;
                    }
                    self.walk_args(&args, depth);
                    return None;
                }
                // Not a router: a package-qualified function or a method of a scanned type.
                let key = self.cx.find_fn(&self.f().dir.clone(), Some(op_text), f);
                self.call_fn(key, &args, depth)
            }
            "identifier" => {
                let name = text(func, src);
                let key = self.cx.find_fn(&self.f().dir.clone(), None, name);
                self.call_fn(key, &args, depth)
            }
            "func_literal" => {
                self.walk_closure(func, None, depth);
                None
            }
            _ => {
                self.walk_args(&args, depth);
                None
            }
        }
    }

    fn walk_args(&mut self, args: &[Node], depth: usize) {
        for a in args {
            self.eval_expr(*a, depth + 1);
        }
    }

    fn call_fn(&mut self, key: Option<FnKey>, args: &[Node], depth: usize) -> Option<usize> {
        let vals: Vec<Option<usize>> = args.iter().map(|a| self.eval_expr(*a, depth + 1)).collect();
        let key = key?;
        self.inline(key, &vals)
    }

    /// Heuristics deciding that an unknown receiver (`s.router`, `mux`, `http`) is a router.
    fn auto_root_ok(&self, recv: &str, method: &str, args: &[Node]) -> bool {
        let src = self.src();
        let lit_at = |i: usize| args.get(i).copied().filter(|n| is_string_lit(*n));
        let path_ok = |n: Node| {
            let s = lit_string(n, src).unwrap_or_default();
            s.starts_with('/') || s.starts_with('*') || s.is_empty()
        };
        let fw = self.fw();
        match method {
            "Handle" | "HandleFunc" => {
                args.len() >= 2
                    && lit_at(0)
                        .map(|n| {
                            let s = lit_string(n, src).unwrap_or_default();
                            s.starts_with('/')
                                || s.split_whitespace()
                                    .next()
                                    .is_some_and(|m| HTTP_METHOD_TOKENS.contains(&m))
                                || (recv != "http" && !s.is_empty())
                        })
                        .unwrap_or(false)
            }
            m if recv != "http" && fw == Fw::Chi && CHI_METHODS.contains(&m) => {
                args.len() >= 2 && lit_at(0).is_some_and(path_ok)
            }
            "Method" | "MethodFunc" if recv != "http" && fw == Fw::Chi => {
                args.len() >= 3 && lit_at(1).is_some_and(path_ok)
            }
            "Route" | "Mount" if recv != "http" && fw == Fw::Chi => {
                args.len() == 2 && lit_at(0).is_some_and(path_ok)
            }
            "Group" if recv != "http" && fw == Fw::Chi => {
                args.len() == 1 && args[0].kind() == "func_literal"
            }
            m if recv != "http" && fw == Fw::Echo && (ECHO_METHODS.contains(&m) || m == "Any") => {
                args.len() >= 2 && lit_at(0).is_some_and(path_ok)
            }
            "Match" | "Add" if recv != "http" && fw == Fw::Echo => {
                args.len() >= 3 && lit_at(1).is_some_and(path_ok)
            }
            "Group" if recv != "http" && fw == Fw::Echo => {
                !args.is_empty() && lit_at(0).is_some_and(path_ok)
            }
            _ => false,
        }
    }

    /// Returns `Some(result)` when `f` was handled as a router method.
    fn router_call(
        &mut self,
        rid: usize,
        f: &str,
        args: &[Node],
        call: Node,
        depth: usize,
    ) -> Option<Option<usize>> {
        let fw = self.fw();
        let src = self.src();
        match f {
            "With" => {
                self.walk_args(args, depth);
                return Some(Some(rid));
            }
            "Use" | "Pre" => {
                self.walk_args(args, depth);
                return Some(None);
            }
            "Handle" | "HandleFunc" => {
                let pat = args.first().map(|a| self.path_or_dyn(Some(*a)))?;
                if let Some(target) = args.get(1) {
                    if let Some(child) = self.mount_target(*target, depth) {
                        self.g.edges.push((rid, mount_prefix(&pat), child));
                        return Some(None);
                    }
                }
                let (method, path) = split_pattern(&pat);
                self.add_route(
                    rid,
                    method.unwrap_or("GET"),
                    path,
                    call,
                    args.get(1).copied(),
                );
                return Some(None);
            }
            "Mount" if fw != Fw::Echo => {
                let pat = args.first().map(|a| self.path_or_dyn(Some(*a)))?;
                if let Some(t) = args.get(1) {
                    if let Some(child) = self.mount_target(*t, depth) {
                        self.g.edges.push((rid, mount_prefix(&pat), child));
                    }
                }
                return Some(None);
            }
            "Route" if fw == Fw::Chi => {
                let pat = args.first().map(|a| self.path_or_dyn(Some(*a)))?;
                let child = self.child(rid, pat);
                if let Some(cb) = args.get(1) {
                    self.run_callback(*cb, child, depth);
                }
                return Some(Some(child));
            }
            "Group" => {
                if args.first().is_some_and(|a| a.kind() == "func_literal") {
                    let child = self.child(rid, String::new());
                    self.run_callback(args[0], child, depth);
                    return Some(Some(child));
                }
                if fw == Fw::Echo {
                    let pat = self.path_or_dyn(args.first().copied());
                    let child = self.child(rid, pat);
                    for a in args.iter().skip(1) {
                        self.eval_expr(*a, depth + 1);
                    }
                    return Some(Some(child));
                }
                return None;
            }
            _ => {}
        }
        if fw == Fw::Chi && CHI_METHODS.contains(&f) {
            let path = self.path_or_dyn(args.first().copied());
            self.add_route(rid, f, path, call, args.get(1).copied());
            return Some(None);
        }
        if fw == Fw::Chi && (f == "Method" || f == "MethodFunc") {
            let m = args.first().and_then(|a| lit_string(*a, src));
            let path = self.path_or_dyn(args.get(1).copied());
            if let Some(m) = m {
                self.add_route(rid, &m, path, call, args.get(2).copied());
            }
            return Some(None);
        }
        if fw == Fw::Echo {
            if ECHO_METHODS.contains(&f) {
                let path = self.path_or_dyn(args.first().copied());
                self.add_route(rid, f, path, call, args.get(1).copied());
                return Some(None);
            }
            match f {
                "Any" => {
                    let path = self.path_or_dyn(args.first().copied());
                    self.add_route(rid, "GET", path, call, args.get(1).copied());
                    return Some(None);
                }
                "Add" => {
                    let m = args.first().and_then(|a| lit_string(*a, src));
                    let path = self.path_or_dyn(args.get(1).copied());
                    if let Some(m) = m {
                        self.add_route(rid, &m, path, call, args.get(2).copied());
                    }
                    return Some(None);
                }
                "Match" => {
                    let mut methods = vec![];
                    if let Some(a) = args.first() {
                        collect_strings(*a, src, &mut methods, 0);
                    }
                    let path = self.path_or_dyn(args.get(1).copied());
                    for m in methods {
                        self.add_route(rid, &m, path.clone(), call, args.get(2).copied());
                    }
                    return Some(None);
                }
                _ => {}
            }
        }
        None
    }

    /// Router designated by the second argument of `Mount` / `Handle`.
    fn mount_target(&mut self, n: Node, depth: usize) -> Option<usize> {
        let src = self.src();
        match n.kind() {
            "call_expression" => {
                let f = n.child_by_field_name("function")?;
                if text(f, src) == "http.StripPrefix" {
                    let args = Self::call_args(n);
                    return self.mount_target(*args.get(1)?, depth + 1);
                }
                self.eval_call(n, depth + 1)
            }
            "identifier" | "selector_expression" => self.env.get(text(n, src)).copied(),
            "parenthesized_expression" => self.mount_target(n.named_child(0)?, depth + 1),
            "unary_expression" | "composite_literal" => self.eval_expr(n, depth + 1),
            _ => None,
        }
    }

    /// Callback of `Route` / `Group`: a closure or the name of a function taking the router.
    fn run_callback(&mut self, cb: Node, router: usize, depth: usize) {
        let src = self.src();
        match cb.kind() {
            "func_literal" => self.walk_closure(cb, Some(router), depth),
            "identifier" | "selector_expression" => {
                let t = text(cb, src);
                let (qual, name) = match t.rsplit_once('.') {
                    Some((q, n)) => (Some(q), n),
                    None => (None, t),
                };
                let key = self.cx.find_fn(&self.f().dir.clone(), qual, name);
                if let Some(key) = key {
                    self.inline(key, &[Some(router)]);
                }
            }
            _ => {}
        }
    }
}

fn collect_strings(n: Node, src: &str, out: &mut Vec<String>, depth: usize) {
    if depth > 8 {
        return;
    }
    if is_string_lit(n) {
        if let Some(s) = lit_string(n, src) {
            out.push(s);
        }
        return;
    }
    let mut c = n.walk();
    for ch in n.named_children(&mut c) {
        collect_strings(ch, src, out, depth + 1);
    }
}

/// A pattern used as a mount point: drop an optional method / host and keep the path.
fn mount_prefix(pat: &str) -> String {
    if pat.contains(DYN) {
        return DYN.to_string();
    }
    split_pattern(pat).1
}

/// Split a net/http pattern (`"GET /users/{id}"`, `"example.com/x"`, `"/x/{$}"`) into
/// an optional method and a host-less path.
fn split_pattern(pat: &str) -> (Option<&'static str>, String) {
    if pat.contains(DYN) {
        return (None, DYN.to_string());
    }
    let pat = pat.trim();
    let mut method = None;
    let mut rest = pat;
    if let Some(i) = pat.find(char::is_whitespace) {
        let m = &pat[..i];
        if let Some(known) = HTTP_METHOD_TOKENS.iter().find(|k| **k == m) {
            method = Some(*known);
            rest = pat[i..].trim();
        }
    }
    if !rest.starts_with('/') {
        rest = match rest.find('/') {
            Some(i) => &rest[i..],
            None => "/",
        };
    }
    let path = rest.replace("{$}", "");
    (method, path)
}
