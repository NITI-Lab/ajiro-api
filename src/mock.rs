//! Mock server. (OWNER: integrations agent)
//!
//! Serves `mock.default` / `mock.scenarios` of the endpoint YAML files.
//! Endpoint files are re-read whenever one of them changes (checked per
//! request). The structure (`Registry` -> `select_response` -> `render`) is
//! kept small and separate so Phase 2 can add stateful behaviour, request
//! validation or recorded responses without touching the HTTP plumbing.

use crate::cli::MockArgs;
use crate::config::Project;
use crate::model::{EndpointDef, MockResponse};
use crate::project::{endpoint_files, load_endpoints};
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};
use tiny_http::{Header, Method, Response, Server, StatusCode};

#[derive(Debug, Clone)]
pub struct MockOptions {
    pub host: String,
    pub port: u16,
    /// Default scenario for every request (header / query override it).
    pub scenario: Option<String>,
    /// Extra latency added to every response.
    pub delay_ms: Option<u64>,
}

impl Default for MockOptions {
    fn default() -> Self {
        MockOptions {
            host: "127.0.0.1".into(),
            port: 4010,
            scenario: None,
            delay_ms: None,
        }
    }
}

// ------------------------------------------------------------- registry ---

#[derive(Debug, Clone)]
enum Seg {
    Lit(String),
    Param(String),
}

#[derive(Debug, Clone)]
struct Route {
    def: EndpointDef,
    rel_path: String,
    segs: Vec<Seg>,
}

impl Route {
    fn method(&self) -> String {
        self.def.request.method.to_ascii_uppercase()
    }
}

fn parse_segs(path: &str) -> Vec<Seg> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.len() > 2 && s.starts_with('{') && s.ends_with('}') {
                Seg::Param(s[1..s.len() - 1].to_string())
            } else {
                Seg::Lit(s.to_string())
            }
        })
        .collect()
}

type Fingerprint = Vec<(PathBuf, Option<SystemTime>, u64)>;

fn fingerprint(api_dir: &Path) -> Fingerprint {
    endpoint_files(api_dir)
        .into_iter()
        .map(|p| {
            let md = std::fs::metadata(&p).ok();
            (
                p,
                md.as_ref().and_then(|m| m.modified().ok()),
                md.map(|m| m.len()).unwrap_or(0),
            )
        })
        .collect()
}

struct Registry {
    routes: Arc<Vec<Route>>,
    fp: Fingerprint,
}

struct Shared {
    project: Project,
    opts: MockOptions,
    registry: Mutex<Registry>,
}

fn load_routes(project: &Project, log: bool) -> Vec<Route> {
    let (eps, errs) = load_endpoints(project);
    if log {
        for e in &errs {
            eprintln!("warning: {}: {}", e.file, e.message);
        }
    }
    eps.into_iter()
        .map(|e| Route {
            segs: parse_segs(&e.def.request.path),
            rel_path: e.rel_path,
            def: e.def,
        })
        .collect()
}

impl Shared {
    /// Current routes; reloads first when any endpoint file changed.
    fn routes(&self) -> Arc<Vec<Route>> {
        let fp = fingerprint(&self.project.api_dir());
        let mut reg = self.registry.lock().unwrap();
        if reg.fp != fp {
            let routes = load_routes(&self.project, true);
            eprintln!("reloaded: {} endpoints", routes.len());
            reg.routes = Arc::new(routes);
            reg.fp = fp;
        }
        reg.routes.clone()
    }
}

/// Find the best route: static segments win over parameters (earlier
/// segments weigh more). HEAD falls back to GET.
fn find_route<'a>(
    routes: &'a [Route],
    method: &str,
    segs: &[String],
) -> Option<(&'a Route, HashMap<String, String>)> {
    let try_method = |m: &str| -> Option<(&'a Route, HashMap<String, String>)> {
        let mut best: Option<(Vec<bool>, &Route, HashMap<String, String>)> = None;
        for r in routes.iter().filter(|r| r.method() == m) {
            if r.segs.len() != segs.len() {
                continue;
            }
            let mut params = HashMap::new();
            let mut rank = Vec::new();
            let mut ok = true;
            for (s, actual) in r.segs.iter().zip(segs) {
                match s {
                    Seg::Lit(l) => {
                        if l != actual {
                            ok = false;
                            break;
                        }
                        rank.push(true);
                    }
                    Seg::Param(n) => {
                        params.insert(n.clone(), actual.clone());
                        rank.push(false);
                    }
                }
            }
            if ok && best.as_ref().map(|b| rank > b.0).unwrap_or(true) {
                best = Some((rank, r, params));
            }
        }
        best.map(|(_, r, p)| (r, p))
    };
    try_method(method).or_else(|| {
        if method == "HEAD" {
            try_method("GET")
        } else {
            None
        }
    })
}

// ------------------------------------------------------------ templating ---

struct Ctx {
    params: HashMap<String, String>,
    query: Vec<(String, String)>,
    headers: HashMap<String, String>,
    body: Option<Value>,
}

/// UTC timestamp `YYYY-MM-DDTHH:MM:SS.mmmZ` for "now" (no chrono dependency).
pub fn iso_now() -> String {
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year,
        month,
        day,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        d.subsec_millis()
    )
}

fn json_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        cur = match cur {
            Value::Object(o) => o.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn resolve_expr(expr: &str, ctx: &Ctx) -> Option<Value> {
    let expr = expr.trim();
    match expr {
        "$uuid" => return Some(json!(uuid::Uuid::new_v4().to_string())),
        "$isoDate" => return Some(json!(iso_now())),
        "$timestamp" => {
            let s = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            return Some(json!(s));
        }
        "body" => return ctx.body.clone(),
        _ => {}
    }
    if let Some(n) = expr.strip_prefix("params.") {
        return ctx.params.get(n).map(|s| json!(s));
    }
    if let Some(n) = expr.strip_prefix("query.") {
        return ctx
            .query
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| json!(v));
    }
    if let Some(n) = expr.strip_prefix("headers.") {
        return ctx.headers.get(&n.to_ascii_lowercase()).map(|s| json!(s));
    }
    if let Some(p) = expr.strip_prefix("body.") {
        return ctx.body.as_ref().and_then(|b| json_path(b, p)).cloned();
    }
    None
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn render_str(s: &str, ctx: &Ctx) -> Value {
    if !s.contains("{{") {
        return Value::String(s.to_string());
    }
    // A string that is exactly one template keeps the JSON type of the value.
    let t = s.trim();
    if t.starts_with("{{") && t.ends_with("}}") && t.matches("{{").count() == 1 {
        let inner = &t[2..t.len() - 2];
        if !inner.contains("}}") {
            return resolve_expr(inner, ctx).unwrap_or_else(|| Value::String(String::new()));
        }
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("}}") {
            Some(end) => {
                let expr = &rest[start + 2..start + 2 + end];
                if let Some(v) = resolve_expr(expr, ctx) {
                    out.push_str(&scalar_text(&v));
                }
                rest = &rest[start + 2 + end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    Value::String(out)
}

fn render(v: &Value, ctx: &Ctx) -> Value {
    match v {
        Value::String(s) => render_str(s, ctx),
        Value::Array(a) => Value::Array(a.iter().map(|x| render(x, ctx)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), render(x, ctx)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

// ------------------------------------------------------------- selection ---

/// Pick the response and its label (`default` or the scenario name).
fn select_response<'a>(
    def: &'a EndpointDef,
    scenario: Option<&str>,
) -> Option<(&'a MockResponse, String)> {
    let mock = def.mock.as_ref()?;
    if let Some(name) = scenario {
        if let Some(r) = mock.scenarios.get(name) {
            return Some((r, name.to_string()));
        }
    }
    mock.default.as_ref().map(|r| (r, "default".to_string()))
}

// ----------------------------------------------------------- HTTP plumbing ---

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_pairs(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(p), String::new()),
        })
        .collect()
}

fn header(name: &str, value: &str) -> Option<Header> {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
}

struct Out {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn json_out(status: u16, v: &Value) -> Out {
    Out {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: serde_json::to_vec_pretty(v).unwrap_or_default(),
    }
}

fn cors_headers(req_headers: &HashMap<String, String>) -> Vec<(String, String)> {
    vec![
        ("Access-Control-Allow-Origin".into(), "*".into()),
        (
            "Access-Control-Allow-Methods".into(),
            "GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD".into(),
        ),
        (
            "Access-Control-Allow-Headers".into(),
            req_headers
                .get("access-control-request-headers")
                .cloned()
                .unwrap_or_else(|| "*".into()),
        ),
        ("Access-Control-Expose-Headers".into(), "*".into()),
        ("Access-Control-Max-Age".into(), "86400".into()),
    ]
}

fn describe(r: &Route) -> String {
    format!("{} {}", r.method(), r.def.request.path)
}

/// Result of handling one request (before it is written to the socket).
struct Handled {
    out: Out,
    /// Log suffix: endpoint id and scenario label.
    what: String,
    delay: Duration,
}

fn handle(
    shared: &Shared,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body_text: &str,
) -> Handled {
    let (path_raw, query_raw) = url.split_once('?').unwrap_or((url, ""));
    let path_raw = path_raw.split('#').next().unwrap_or(path_raw);
    let query = parse_pairs(query_raw);
    let segs: Vec<String> = path_raw
        .split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect();

    let done = |out: Out, what: &str| Handled {
        out,
        what: what.to_string(),
        delay: Duration::ZERO,
    };

    let routes = shared.routes();

    if path_raw == "/__ajiro/endpoints" && method == "GET" {
        let list: Vec<Value> = routes
            .iter()
            .map(|r| {
                json!({
                    "id": r.def.id,
                    "method": r.method(),
                    "path": r.def.request.path,
                    "file": r.rel_path,
                    "has_mock": r.def.mock.as_ref().map(|m| m.default.is_some()).unwrap_or(false),
                    "scenarios": r.def.mock.as_ref().map(|m| m.scenarios.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                })
            })
            .collect();
        return done(json_out(200, &json!({ "endpoints": list })), "(ajiro)");
    }

    let found = find_route(&routes, method, &segs);

    if found.is_none() && method == "OPTIONS" {
        return done(
            Out {
                status: 204,
                headers: vec![],
                body: vec![],
            },
            "(preflight)",
        );
    }

    let (route, params) = match found {
        Some(f) => f,
        None => {
            let first = segs.first().cloned();
            let close: Vec<String> = routes
                .iter()
                .filter(|r| {
                    let same_path = r.segs.len() == segs.len()
                        && r.segs.iter().zip(&segs).all(|(s, a)| match s {
                            Seg::Lit(l) => l == a,
                            Seg::Param(_) => true,
                        });
                    let same_first = first.is_some()
                        && matches!(r.segs.first(), Some(Seg::Lit(l)) if Some(l) == first.as_ref());
                    same_path || same_first
                })
                .take(8)
                .map(describe)
                .collect();
            let v = json!({
                "error": "no matching endpoint",
                "method": method,
                "path": format!("/{}", segs.join("/")),
                "close_matches": close,
                "hint": "GET /__ajiro/endpoints lists every served endpoint",
            });
            return done(json_out(404, &v), "(no route)");
        }
    };

    let scenario = headers
        .get("x-ajiro-scenario")
        .cloned()
        .or_else(|| {
            query
                .iter()
                .find(|(k, _)| k == "__scenario")
                .map(|(_, v)| v.clone())
        })
        .or_else(|| shared.opts.scenario.clone());

    let (resp, label) = match select_response(&route.def, scenario.as_deref()) {
        Some(x) => x,
        None => {
            let v = json!({
                "error": "no mock defined",
                "endpoint": route.def.id,
                "hint": format!("add mock.default to {}", route.rel_path),
            });
            return Handled {
                out: json_out(501, &v),
                what: format!("{} [no mock]", route.def.id),
                delay: Duration::ZERO,
            };
        }
    };

    let body_json: Option<Value> = if body_text.trim().is_empty() {
        None
    } else if let Ok(v) = serde_json::from_str::<Value>(body_text) {
        Some(v)
    } else if headers
        .get("content-type")
        .map(|c| c.contains("x-www-form-urlencoded"))
        .unwrap_or(false)
    {
        let mut m = Map::new();
        for (k, v) in parse_pairs(body_text) {
            m.insert(k, Value::String(v));
        }
        Some(Value::Object(m))
    } else {
        None
    };
    let ctx = Ctx {
        params,
        query: query.clone(),
        headers: headers.clone(),
        body: body_json,
    };

    let mut out_headers: Vec<(String, String)> = Vec::new();
    for (k, v) in &resp.headers {
        out_headers.push((k.clone(), scalar_text(&render_str(v, &ctx))));
    }
    let has_ct = out_headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    let body = match &resp.body {
        None => vec![],
        Some(Value::String(s)) => {
            if !has_ct {
                out_headers.push(("Content-Type".into(), "text/plain; charset=utf-8".into()));
            }
            scalar_text(&render_str(s, &ctx)).into_bytes()
        }
        Some(v) => {
            if !has_ct {
                out_headers.push(("Content-Type".into(), "application/json".into()));
            }
            serde_json::to_vec_pretty(&render(v, &ctx)).unwrap_or_default()
        }
    };
    let delay =
        Duration::from_millis(resp.delay_ms.unwrap_or(0) + shared.opts.delay_ms.unwrap_or(0));
    Handled {
        out: Out {
            status: resp.status,
            headers: out_headers,
            body,
        },
        what: format!("{} [{}]", route.def.id, label),
        delay,
    }
}

fn serve_request(shared: &Shared, mut req: tiny_http::Request) {
    let start = Instant::now();
    let method = match req.method() {
        Method::Get => "GET".to_string(),
        Method::Post => "POST".into(),
        Method::Put => "PUT".into(),
        Method::Patch => "PATCH".into(),
        Method::Delete => "DELETE".into(),
        Method::Head => "HEAD".into(),
        Method::Options => "OPTIONS".into(),
        other => other.as_str().to_ascii_uppercase(),
    };
    let url = req.url().to_string();
    let headers: HashMap<String, String> = req
        .headers()
        .iter()
        .map(|h| {
            (
                h.field.as_str().as_str().to_ascii_lowercase(),
                h.value.as_str().to_string(),
            )
        })
        .collect();
    let mut body_text = String::new();
    let _ = req
        .as_reader()
        .take(8 * 1024 * 1024)
        .read_to_string(&mut body_text);

    let h = handle(shared, &method, &url, &headers, &body_text);
    if !h.delay.is_zero() {
        std::thread::sleep(h.delay);
    }
    let status = h.out.status;
    let mut resp = Response::from_data(h.out.body).with_status_code(StatusCode(status));
    for (k, v) in cors_headers(&headers).into_iter().chain(h.out.headers) {
        if let Some(hd) = header(&k, &v) {
            resp.add_header(hd);
        }
    }
    let _ = req.respond(resp);
    let path = url.split('?').next().unwrap_or(&url);
    eprintln!(
        "{} {} \u{2192} {} {} {}ms",
        method,
        path,
        status,
        h.what,
        start.elapsed().as_millis()
    );
}

// ------------------------------------------------------------- lifecycle ---

/// A running mock server. Dropping the handle stops it.
pub struct MockHandle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    endpoints: usize,
}

impl MockHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
    /// Number of endpoints loaded at startup.
    pub fn endpoint_count(&self) -> usize {
        self.endpoints
    }
    /// Block until the server thread ends (normally: forever).
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for MockHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Bind and start serving in a background thread. Use `port: 0` to pick a
/// free port (see [`MockHandle::port`]).
pub fn start(project: Project, opts: MockOptions) -> Result<MockHandle> {
    let bind = format!("{}:{}", opts.host, opts.port);
    let server = Server::http(&bind).map_err(|e| anyhow!("cannot listen on {}: {}", bind, e))?;
    let addr = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("mock server has no IP address"))?;
    let routes = load_routes(&project, true);
    let endpoints = routes.len();
    let fp = fingerprint(&project.api_dir());
    let shared = Arc::new(Shared {
        project,
        opts,
        registry: Mutex::new(Registry {
            routes: Arc::new(routes),
            fp,
        }),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let thread = std::thread::spawn(move || {
        while !stop2.load(Ordering::SeqCst) {
            match server.recv_timeout(Duration::from_millis(100)) {
                Ok(Some(req)) => {
                    let s = shared.clone();
                    std::thread::spawn(move || serve_request(&s, req));
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });
    Ok(MockHandle {
        addr,
        stop,
        thread: Some(thread),
        endpoints,
    })
}

/// Blocking variant of [`start`].
pub fn serve(project: Project, opts: MockOptions) -> Result<()> {
    start(project, opts)?.wait();
    Ok(())
}

pub fn run_cli(dir: &Path, args: &MockArgs) -> Result<i32> {
    let project = Project::discover_or_default(dir)?;
    let handle = start(
        project,
        MockOptions {
            host: args.host.clone(),
            port: args.port,
            scenario: args.scenario.clone(),
            delay_ms: args.delay,
        },
    )?;
    println!(
        "Ajiro mock server: {} endpoints on {}",
        handle.endpoint_count(),
        handle.url()
    );
    println!("  GET {}/__ajiro/endpoints lists them; select a scenario with the X-Ajiro-Scenario header or ?__scenario=", handle.url());
    if handle.endpoint_count() == 0 {
        println!("  (no endpoints yet: run `ajiro-api scan` or import an OpenAPI file)");
    }
    handle.wait();
    Ok(crate::exit::OK)
}
