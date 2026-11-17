//! `test`: run endpoint requests and check responses.
//!
//! Layout: `template` (variables), `http` (ureq wrapper), `assert` (checks),
//! `jsonpath` (query / remove / diff), `redact` (secret masking),
//! `output` (human + JUnit).

mod assert;
mod http;
mod jsonpath;
mod output;
mod redact;
mod template;

use crate::cli::TestArgs;
use crate::config::Project;
use crate::model::{EndpointDef, Expect, TestCase};
use crate::naming::slug;
use crate::project::{load_endpoints, validate, LoadedEndpoint};
use crate::report::{
    Failure, RequestLog, ResponseLog, TestOptions, TestReport, TestResult, TestStatus, TestSummary,
    REPORT_VERSION,
};
use anyhow::{anyhow, bail, Context as _, Result};
use indexmap::IndexMap;
use redact::Redactor;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;
use template::{render_text, render_value, Vars};

/// Maximum size of a response body stored in the report.
const MAX_BODY_LOG: usize = 64 * 1024;

/// Run all tests of a project. Never prints.
pub fn run_tests(project: &Project, opts: &TestOptions) -> Result<TestReport> {
    Ok(run_inner(project, opts)?.0)
}

/// CLI entry: run, print (human or `--json`), write JUnit, return the exit code.
pub fn run_cli(dir: &Path, args: &TestArgs) -> Result<i32> {
    let project = Project::discover(dir)?;
    let opts = TestOptions {
        env: args.env.clone(),
        filter: args.filter.clone(),
        update_snapshots: args.update_snapshots,
        bail: args.bail,
        base_url: args.base_url.clone(),
        timeout_ms: args.timeout,
        smoke_unsafe: args.smoke_unsafe,
    };
    let (report, notes) = run_inner(&project, &opts)?;
    if let Some(path) = &args.junit {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
        }
        std::fs::write(path, output::junit(&report))
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", output::human(&report, args.verbose, &notes));
    }
    Ok(if report.ok {
        crate::exit::OK
    } else {
        crate::exit::FAILURE
    })
}

// ------------------------------------------------------------------ plan ---

struct Planned {
    index: usize,
    test: TestCase,
    smoke: bool,
}

enum Sel {
    All,
    Tests(Vec<usize>),
}

fn effective_tests(def: &EndpointDef) -> Vec<Planned> {
    if def.tests.is_empty() {
        return vec![Planned {
            index: 0,
            test: TestCase {
                name: "smoke".into(),
                ..Default::default()
            },
            smoke: true,
        }];
    }
    def.tests
        .iter()
        .enumerate()
        .map(|(index, t)| Planned {
            index,
            test: t.clone(),
            smoke: false,
        })
        .collect()
}

/// Indexes of the `setup` endpoints and, transitively, the endpoints they
/// depend on.
fn setup_closure(eps: &[LoadedEndpoint], setup: &[String]) -> Result<HashSet<usize>> {
    let ids: HashMap<&str, usize> = eps
        .iter()
        .enumerate()
        .map(|(i, e)| (e.def.id.as_str(), i))
        .collect();
    let mut out = HashSet::new();
    let mut stack: Vec<usize> = Vec::new();
    for id in setup {
        match ids.get(id.as_str()) {
            Some(&i) => stack.push(i),
            None => bail!("unknown endpoint id `{}`", id),
        }
    }
    while let Some(i) = stack.pop() {
        if out.insert(i) {
            for d in &eps[i].def.depends_on {
                if let Some(&j) = ids.get(d.as_str()) {
                    stack.push(j);
                }
            }
        }
    }
    Ok(out)
}

/// Topological order by `depends_on`, ties broken by file order.
fn order(eps: &[LoadedEndpoint]) -> Result<Vec<usize>> {
    let ids: HashMap<&str, usize> = eps
        .iter()
        .enumerate()
        .map(|(i, e)| (e.def.id.as_str(), i))
        .collect();
    let mut done = vec![false; eps.len()];
    let mut out = Vec::with_capacity(eps.len());
    while out.len() < eps.len() {
        let next = (0..eps.len()).find(|&i| {
            !done[i]
                && eps[i]
                    .def
                    .depends_on
                    .iter()
                    .all(|d| ids.get(d.as_str()).map_or(true, |&j| j == i || done[j]))
        });
        match next {
            Some(i) => {
                done[i] = true;
                out.push(i);
            }
            None => {
                let stuck: Vec<&str> = (0..eps.len())
                    .filter(|&i| !done[i])
                    .map(|i| eps[i].def.id.as_str())
                    .collect();
                bail!(
                    "depends_on cycle detected among endpoints: {}",
                    stuck.join(", ")
                );
            }
        }
    }
    Ok(out)
}

/// Which endpoints / tests run for `filter`. Dependencies of selected
/// endpoints are included (all their tests) so that captures are available.
fn select(eps: &[LoadedEndpoint], filter: Option<&str>) -> Vec<Option<Sel>> {
    let Some(f) = filter.map(|f| f.to_lowercase()).filter(|f| !f.is_empty()) else {
        return eps.iter().map(|_| Some(Sel::All)).collect();
    };
    let mut sel: Vec<Option<Sel>> = eps
        .iter()
        .map(|e| {
            if e.def.id.to_lowercase().contains(&f) || e.rel_path.to_lowercase().contains(&f) {
                return Some(Sel::All);
            }
            let hits: Vec<usize> = effective_tests(&e.def)
                .iter()
                .filter(|p| p.test.name.to_lowercase().contains(&f))
                .map(|p| p.index)
                .collect();
            if hits.is_empty() {
                None
            } else {
                Some(Sel::Tests(hits))
            }
        })
        .collect();
    let ids: HashMap<&str, usize> = eps
        .iter()
        .enumerate()
        .map(|(i, e)| (e.def.id.as_str(), i))
        .collect();
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..eps.len() {
            if sel[i].is_none() {
                continue;
            }
            for d in &eps[i].def.depends_on {
                if let Some(&j) = ids.get(d.as_str()) {
                    if sel[j].is_none() {
                        sel[j] = Some(Sel::All);
                        changed = true;
                    }
                }
            }
        }
    }
    sel
}

// --------------------------------------------------------------- request ---

struct Built {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body_bytes: Option<Vec<u8>>,
    body: Option<Value>,
}

/// Methods the implicit smoke test may send without asking (RFC 9110 safe
/// methods).
fn is_safe_method(method: &str) -> bool {
    matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// Value of the cookie `name` from the response `Set-Cookie` headers (the
/// last one wins, as in a browser).
fn set_cookie_value(resp: &http::Response, name: &str) -> Option<String> {
    resp.headers
        .iter()
        .rev()
        .filter(|(k, _)| k == "set-cookie")
        .find_map(|(_, v)| {
            let pair = v.split(';').next()?.trim();
            let (n, val) = pair.split_once('=')?;
            (n.trim() == name).then(|| val.trim().trim_matches('"').to_string())
        })
}

fn set_header(headers: &mut Vec<(String, String)>, k: &str, v: &str) {
    match headers.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(k)) {
        Some(slot) => *slot = (k.to_string(), v.to_string()),
        None => headers.push((k.to_string(), v.to_string())),
    }
}

fn deep_merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                if v.is_null() {
                    b.shift_remove(k);
                } else if let (Some(slot), true) = (b.get_mut(k), v.is_object()) {
                    if slot.is_object() {
                        deep_merge(slot, v);
                    } else {
                        *slot = strip_nulls(v);
                    }
                } else {
                    b.insert(k.clone(), strip_nulls(v));
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// A freshly inserted override object: nested `null`s mean "delete", which is
/// a no-op when there is nothing to delete.
fn strip_nulls(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(_, x)| !x.is_null())
                .map(|(k, x)| (k.clone(), strip_nulls(x)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn pct_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && b == b'/')
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn query_pairs(query: &IndexMap<String, Value>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (k, v) in query {
        match v {
            Value::Null => {}
            Value::Array(items) => {
                for i in items.iter().filter(|i| !i.is_null()) {
                    out.push((k.clone(), scalar_text(i)));
                }
            }
            other => out.push((k.clone(), scalar_text(other))),
        }
    }
    out
}

fn render_expect(e: &Expect, vars: &Vars) -> Result<Expect, template::TemplateError> {
    let mut o = e.clone();
    for v in o.headers.values_mut() {
        *v = render_text(v, vars)?;
    }
    if let Some(b) = &mut o.body {
        if let Some(eq) = &b.equals {
            b.equals = Some(render_value(eq, vars)?);
        }
        let mut rendered = indexmap::IndexMap::new();
        for (k, v) in &b.match_ {
            // Templates are allowed in JSONPath keys too, e.g.
            // `$.items[?(@.id == '{{ todoId }}')].title`.
            rendered.insert(render_text(k, vars)?, render_value(v, vars)?);
        }
        b.match_ = rendered;
        if let Some(c) = &b.contains {
            b.contains = Some(render_text(c, vars)?);
        }
    }
    Ok(o)
}

// ------------------------------------------------------------------- run ---

struct Run<'a> {
    project: &'a Project,
    opts: &'a TestOptions,
    env_name: String,
    base_url: String,
    /// Environment headers as templates; rendered per request so they can
    /// use values captured by `setup` endpoints.
    env_headers: Vec<(String, String)>,
    agent: ureq::Agent,
    vars: Vars,
    redactor: Redactor,
    /// True while the environment `setup` endpoints run.
    in_setup: bool,
}

fn load_dotenv(path: &Path, into: &mut HashMap<String, String>) {
    if let Ok(iter) = dotenvy::from_path_iter(path) {
        for (k, v) in iter.flatten() {
            into.insert(k, v);
        }
    }
}

fn run_inner(project: &Project, opts: &TestOptions) -> Result<(TestReport, Vec<(usize, String)>)> {
    let started = Instant::now();
    let cfg = &project.config;

    let env_name = opts.env.clone().unwrap_or_else(|| cfg.default_env.clone());
    let env = cfg.environments.get(&env_name).ok_or_else(|| {
        let avail: Vec<&str> = cfg.environments.keys().map(|s| s.as_str()).collect();
        anyhow!(
            "unknown environment `{}`; available environments: {}",
            env_name,
            if avail.is_empty() {
                "(none defined in ajiro-api.yaml)".to_string()
            } else {
                avail.join(", ")
            }
        )
    })?;
    let base_url = opts
        .base_url
        .clone()
        .unwrap_or_else(|| env.base_url.clone())
        .trim_end_matches('/')
        .to_string();

    let mut dotenv = HashMap::new();
    load_dotenv(&project.root.join(&cfg.env_file), &mut dotenv);
    if let Some(f) = &env.env_file {
        load_dotenv(&project.root.join(f), &mut dotenv);
    }
    let redactor = Redactor::new(&cfg.redact, dotenv.values().cloned());

    let mut vars = Vars {
        dotenv,
        vars: IndexMap::new(),
    };
    for (k, v) in &env.variables {
        let rendered = render_value(v, &vars)
            .map_err(|e| anyhow!("environments.{}.variables.{}: {}", env_name, k, e))?;
        vars.vars.insert(k.clone(), rendered);
    }
    let mut env_headers = Vec::new();
    for (k, v) in &env.headers {
        // `{{ env.X }}` must resolve now; other variables may be captured
        // later by `setup` endpoints, so the header stays a template.
        match render_text(v, &vars) {
            Err(template::TemplateError::Undefined(name)) if name.starts_with("env.") => {
                return Err(anyhow!(
                    "environments.{}.headers.{}: {}",
                    env_name,
                    k,
                    template::TemplateError::Undefined(name)
                ));
            }
            _ => set_header(&mut env_headers, k, v),
        }
    }

    let timeout = opts.timeout_ms.unwrap_or(cfg.timeout_ms);
    let agent = http::make_agent(timeout, &base_url);

    let (endpoints, mut load_errors) = load_endpoints(project);
    load_errors.extend(validate(&endpoints));
    let ordered = order(&endpoints)?;
    let selection = select(&endpoints, opts.filter.as_deref());

    let mut run = Run {
        project,
        opts,
        env_name: env_name.clone(),
        base_url: base_url.clone(),
        env_headers,
        agent,
        vars,
        redactor,
        in_setup: false,
    };

    // `setup` endpoints (and what they depend on) run first, always, with
    // all their tests; the rest keeps the `depends_on` order.
    let setup = setup_closure(&endpoints, &env.setup)
        .map_err(|e| anyhow!("environments.{}.setup: {}", env_name, e))?;
    let mut results: Vec<TestResult> = Vec::new();
    let mut notes: Vec<(usize, String)> = Vec::new();
    run.in_setup = true;
    'setup: for &ei in ordered.iter().filter(|i| setup.contains(i)) {
        let ep = &endpoints[ei];
        for planned in effective_tests(&ep.def) {
            let (res, n) = run.run_one(ep, &planned);
            let failed = matches!(res.status, TestStatus::Failed | TestStatus::Error);
            for text in n {
                notes.push((results.len(), text));
            }
            results.push(res);
            if failed {
                notes.push((
                    results.len() - 1,
                    format!(
                        "setup endpoint `{}` failed; the remaining tests were not run",
                        ep.def.id
                    ),
                ));
                break 'setup;
            }
        }
    }
    let setup_failed = results
        .iter()
        .any(|r| matches!(r.status, TestStatus::Failed | TestStatus::Error));
    run.in_setup = false;
    'outer: for ei in ordered {
        if setup_failed {
            break 'outer;
        }
        if setup.contains(&ei) {
            continue;
        }
        let Some(sel) = &selection[ei] else { continue };
        let ep = &endpoints[ei];
        for planned in effective_tests(&ep.def) {
            if let Sel::Tests(ix) = sel {
                if !ix.contains(&planned.index) {
                    continue;
                }
            }
            let (res, n) = run.run_one(ep, &planned);
            let stop = opts.bail && matches!(res.status, TestStatus::Failed | TestStatus::Error);
            for text in n {
                notes.push((results.len(), text));
            }
            results.push(res);
            if stop {
                break 'outer;
            }
        }
    }

    let mut summary = TestSummary {
        total: results.len(),
        ..Default::default()
    };
    for r in &results {
        match r.status {
            TestStatus::Passed => summary.passed += 1,
            TestStatus::Failed => summary.failed += 1,
            TestStatus::Skipped => summary.skipped += 1,
            TestStatus::Error => summary.errors += 1,
        }
    }
    summary.duration_ms = started.elapsed().as_millis() as u64;
    let ok = summary.failed == 0 && summary.errors == 0 && load_errors.is_empty();
    let report = TestReport {
        report_version: REPORT_VERSION,
        env: env_name,
        base_url: run.redactor.mask_str(&base_url),
        ok,
        summary,
        results,
        load_errors,
    };
    Ok((report, notes))
}

impl Run<'_> {
    fn build_request(&self, ep: &LoadedEndpoint, test: &TestCase) -> Result<Built, String> {
        let base = &ep.def.request;
        let ov = test.request.as_ref();
        let ctx = |what: &str, e: template::TemplateError| format!("{} (in {})", e, what);

        let method = ov
            .and_then(|o| o.method.clone())
            .unwrap_or_else(|| base.method.clone())
            .to_ascii_uppercase();
        let path_raw = ov
            .and_then(|o| o.path.clone())
            .unwrap_or_else(|| base.path.clone());
        let path_tpl = render_text(&path_raw, &self.vars).map_err(|e| ctx("request.path", e))?;

        let mut params = base.params.clone();
        let mut query = base.query.clone();
        let mut header_defs: Vec<(String, String)> = self.env_headers.clone();
        for (k, v) in &base.headers {
            set_header(&mut header_defs, k, v);
        }
        let mut body = base.body.clone();
        if let Some(o) = ov {
            params.extend(o.params.clone());
            query.extend(o.query.clone());
            for (k, v) in &o.headers {
                set_header(&mut header_defs, k, v);
            }
            if let Some(ob) = &o.body {
                match &mut body {
                    Some(b) => deep_merge(b, ob),
                    None => body = Some(strip_nulls(ob)),
                }
            }
        }

        // params
        let mut rparams: HashMap<String, Value> = HashMap::new();
        for (k, v) in &params {
            rparams.insert(
                k.clone(),
                render_value(v, &self.vars)
                    .map_err(|e| ctx(&format!("request.params.{}", k), e))?,
            );
        }
        let mut path = String::new();
        let mut rest = path_tpl.as_str();
        while let Some(s) = rest.find('{') {
            let Some(len) = rest[s..].find('}') else {
                break;
            };
            path.push_str(&rest[..s]);
            let name = &rest[s + 1..s + len];
            let value = rparams
                .get(name)
                .cloned()
                .or_else(|| self.vars.get(name))
                .ok_or_else(|| {
                    format!(
                        "path parameter `{{{}}}` has no value: set request.params.{} (or a test override), or define a variable named `{}`",
                        name, name, name
                    )
                })?;
            path.push_str(&pct_encode(&scalar_text(&value), true));
            rest = &rest[s + len + 1..];
        }
        path.push_str(rest);
        if !path.starts_with('/') {
            path.insert(0, '/');
        }

        // query
        let mut rquery: IndexMap<String, Value> = IndexMap::new();
        for (k, v) in &query {
            rquery.insert(
                k.clone(),
                render_value(v, &self.vars).map_err(|e| ctx(&format!("request.query.{}", k), e))?,
            );
        }
        let pairs = query_pairs(&rquery);
        let qs = if pairs.is_empty() {
            String::new()
        } else {
            format!(
                "?{}",
                pairs
                    .iter()
                    .map(|(k, v)| format!("{}={}", pct_encode(k, false), pct_encode(v, false)))
                    .collect::<Vec<_>>()
                    .join("&")
            )
        };

        // headers (an empty value removes the header, e.g. to test a request
        // without `Authorization`)
        let mut headers: Vec<(String, String)> = Vec::new();
        for (k, v) in header_defs.iter().filter(|(_, v)| !v.is_empty()) {
            let from_env = self
                .env_headers
                .iter()
                .any(|(ek, ev)| ek.eq_ignore_ascii_case(k) && ev == v);
            let rv = match render_text(v, &self.vars) {
                Ok(rv) => rv,
                // While `setup` runs, an environment header that needs a value
                // the setup has not captured yet (the session cookie of a
                // login) is left out instead of failing the request.
                Err(template::TemplateError::Undefined(_)) if from_env && self.in_setup => {
                    continue;
                }
                Err(e) if from_env => {
                    return Err(format!(
                        "{} (in environments.{}.headers.{}; capture it in a `setup` endpoint or define it in `variables`)",
                        e, self.env_name, k
                    ));
                }
                Err(e) => return Err(ctx(&format!("request.headers.{}", k), e)),
            };
            headers.push((k.clone(), rv));
        }

        // body
        let rbody = match &body {
            Some(b) => Some(render_value(b, &self.vars).map_err(|e| ctx("request.body", e))?),
            None => None,
        };
        let mut body_bytes = None;
        if let Some(b) = &rbody {
            let ct = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .map(|(_, v)| v.to_ascii_lowercase());
            if ct.is_none() {
                headers.push(("content-type".into(), "application/json".into()));
            }
            body_bytes = Some(match (b, ct.as_deref()) {
                (Value::String(s), _) => s.clone().into_bytes(),
                (Value::Object(m), Some(c)) if c.contains("application/x-www-form-urlencoded") => {
                    let mut form: IndexMap<String, Value> = IndexMap::new();
                    for (k, v) in m {
                        form.insert(k.clone(), v.clone());
                    }
                    query_pairs(&form)
                        .iter()
                        .map(|(k, v)| format!("{}={}", pct_encode(k, false), pct_encode(v, false)))
                        .collect::<Vec<_>>()
                        .join("&")
                        .into_bytes()
                }
                (other, _) => serde_json::to_vec(other).map_err(|e| e.to_string())?,
            });
        }

        Ok(Built {
            method,
            url: format!("{}{}{}", self.base_url, path, qs),
            headers,
            body_bytes,
            body: rbody,
        })
    }

    fn request_log(&self, b: &Built) -> RequestLog {
        let headers: IndexMap<String, String> = b
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), self.redactor.mask_header(k, v)))
            .collect();
        RequestLog {
            method: b.method.clone(),
            url: self.redactor.mask_str(&b.url),
            headers,
            body: b.body.as_ref().map(|v| self.redactor.mask_value(v)),
        }
    }

    fn response_log(&self, r: &http::Response) -> ResponseLog {
        let mut headers: IndexMap<String, String> = IndexMap::new();
        for (k, v) in &r.headers {
            let masked = self.redactor.mask_header(k, v);
            match headers.get_mut(k) {
                Some(existing) => {
                    existing.push_str(", ");
                    existing.push_str(&masked);
                }
                None => {
                    headers.insert(k.clone(), masked);
                }
            }
        }
        let body = if r.text.is_empty() {
            None
        } else {
            let value = match &r.json {
                Some(j) => {
                    let masked = self.redactor.mask_value(j);
                    let ser = masked.to_string();
                    if ser.len() > MAX_BODY_LOG {
                        Value::String(truncate_log(&ser))
                    } else {
                        masked
                    }
                }
                None => Value::String(truncate_log(&self.redactor.mask_str(&r.text))),
            };
            Some(value)
        };
        ResponseLog {
            status: r.status,
            headers,
            body,
        }
    }

    fn run_one(&mut self, ep: &LoadedEndpoint, planned: &Planned) -> (TestResult, Vec<String>) {
        let test = &planned.test;
        let started = Instant::now();
        let mut result = TestResult {
            endpoint_id: ep.def.id.clone(),
            file: ep.rel_path.clone(),
            test: test.name.clone(),
            test_index: planned.index,
            status: TestStatus::Passed,
            duration_ms: 0,
            request: None,
            response: None,
            failures: vec![],
            error: None,
            source: ep.def.source.clone(),
        };
        let skip = |mut r: TestResult, reason: String| {
            r.status = TestStatus::Skipped;
            r.error = Some(reason);
            r
        };
        let err = |mut r: TestResult, msg: String, started: Instant| {
            r.status = TestStatus::Error;
            r.error = Some(msg);
            r.duration_ms = started.elapsed().as_millis() as u64;
            r
        };

        if test.skip {
            return (skip(result, "skip: true".into()), vec![]);
        }
        if planned.smoke && !self.in_setup && !self.opts.smoke_unsafe {
            let method = ep.def.request.method.to_ascii_uppercase();
            if !is_safe_method(&method) {
                let reason = format!(
                    "smoke test skipped: {} may change data; add tests to {} (or run with --smoke-unsafe)",
                    method, ep.rel_path
                );
                return (skip(result, reason), vec![]);
            }
        }
        if !test.only_env.is_empty() && !test.only_env.contains(&self.env_name) {
            let reason = format!(
                "only_env [{}] does not include `{}`",
                test.only_env.join(", "),
                self.env_name
            );
            return (skip(result, reason), vec![]);
        }

        let built = match self.build_request(ep, test) {
            Ok(b) => b,
            Err(msg) => {
                let msg = self.redactor.mask_str(&msg);
                if planned.smoke {
                    return (skip(result, format!("smoke test skipped: {}", msg)), vec![]);
                }
                return (err(result, msg, started), vec![]);
            }
        };
        let expect = match render_expect(&test.expect, &self.vars) {
            Ok(e) => e,
            Err(e) => {
                result.request = Some(self.request_log(&built));
                return (
                    err(
                        result,
                        self.redactor.mask_str(&format!("{} (in expect)", e)),
                        started,
                    ),
                    vec![],
                );
            }
        };
        result.request = Some(self.request_log(&built));

        let req = http::HttpRequest {
            method: built.method.clone(),
            url: built.url.clone(),
            headers: built.headers.clone(),
            body: built.body_bytes.clone(),
        };
        let resp = match http::send(&self.agent, &req) {
            Ok(r) => r,
            Err(http::SendError::ConnectionRefused(m)) => {
                let msg = format!(
                    "{} {} failed: {}. Is the server running at {}?",
                    built.method,
                    result
                        .request
                        .as_ref()
                        .map(|r| r.url.as_str())
                        .unwrap_or(""),
                    m,
                    self.base_url
                );
                return (err(result, self.redactor.mask_str(&msg), started), vec![]);
            }
            Err(http::SendError::Other(m)) => {
                let msg = format!(
                    "{} {} failed: {}",
                    built.method,
                    result
                        .request
                        .as_ref()
                        .map(|r| r.url.as_str())
                        .unwrap_or(""),
                    m
                );
                return (err(result, self.redactor.mask_str(&msg), started), vec![]);
            }
        };

        // assertions
        let mut failures: Vec<Failure>;
        let mut notes: Vec<String> = Vec::new();
        if planned.smoke {
            failures = Vec::new();
            if resp.status >= 500 {
                failures.push(Failure {
                    check: "status".into(),
                    path: None,
                    expected: json!("< 500"),
                    actual: json!(resp.status),
                    message: format!(
                        "got status {}; no tests defined; smoke check only — add tests to {}",
                        resp.status, ep.rel_path
                    ),
                });
            }
        } else {
            let api_dir = self.project.api_dir();
            let snapshot_path = api_dir
                .join("__snapshots__")
                .join(ep.def.id.replace(['/', '\\'], "_"))
                .join(format!("{}.json", slug(&test.name)));
            let endpoint_dir: PathBuf = ep
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| api_dir.clone());
            let ctx = assert::Context {
                expect: &expect,
                resp: &resp,
                redactor: &self.redactor,
                endpoint_dir: &endpoint_dir,
                api_dir: &api_dir,
                snapshot_path: &snapshot_path,
                update_snapshots: self.opts.update_snapshots,
            };
            let out = assert::check(&ctx);
            failures = out.failures;
            notes = out.notes;
            self.capture(test, &resp, &mut failures);
        }

        // finalize: mask, build logs
        for f in &mut failures {
            f.expected = self.redactor.mask_value(&f.expected);
            f.actual = self.redactor.mask_value(&f.actual);
            f.message = self.redactor.mask_str(&f.message);
        }
        result.response = Some(self.response_log(&resp));
        result.status = if failures.is_empty() {
            TestStatus::Passed
        } else {
            TestStatus::Failed
        };
        result.failures = failures;
        result.duration_ms = started.elapsed().as_millis() as u64;
        (result, notes)
    }

    fn capture(&mut self, test: &TestCase, resp: &http::Response, failures: &mut Vec<Failure>) {
        for (name, spec) in &test.capture {
            let value: Result<Option<Value>, String> = if spec == "status" {
                Ok(Some(json!(resp.status)))
            } else if spec == "body" {
                Ok(Some(
                    resp.json
                        .clone()
                        .unwrap_or_else(|| Value::String(resp.text.clone())),
                ))
            } else if let Some(h) = spec.strip_prefix("header:") {
                Ok(resp.header(h.trim()).map(|s| Value::String(s.to_string())))
            } else if let Some(c) = spec.strip_prefix("cookie:") {
                Ok(set_cookie_value(resp, c.trim()).map(Value::String))
            } else {
                match &resp.json {
                    Some(j) => jsonpath::query_first(j, spec).map(|o| o.cloned()),
                    None => Err("response body is not JSON".to_string()),
                }
            };
            match value {
                Ok(Some(v)) => {
                    let sensitive = self.redactor.is_sensitive(name)
                        || (spec.starts_with('$')
                            && jsonpath::last_key(spec)
                                .is_some_and(|k| self.redactor.is_sensitive(&k)))
                        || spec
                            .strip_prefix("header:")
                            .is_some_and(|h| self.redactor.is_sensitive(h.trim()))
                        || spec.starts_with("cookie:");
                    if sensitive {
                        match &v {
                            Value::String(s) => self.redactor.add_secret(s),
                            Value::Null | Value::Bool(_) => {}
                            other => self.redactor.add_secret(&other.to_string()),
                        }
                    }
                    self.vars.vars.insert(name.clone(), v);
                }
                Ok(None) => failures.push(Failure {
                    check: "capture".into(),
                    path: Some(spec.clone()),
                    expected: json!(format!("a value for `{}`", name)),
                    actual: Value::Null,
                    message: format!("capture `{}`: {} not found in the response", name, spec),
                }),
                Err(msg) => failures.push(Failure {
                    check: "capture".into(),
                    path: Some(spec.clone()),
                    expected: json!(format!("a value for `{}`", name)),
                    actual: Value::Null,
                    message: format!("capture `{}`: {}", name, msg),
                }),
            }
        }
    }
}

fn truncate_log(s: &str) -> String {
    if s.len() <= MAX_BODY_LOG {
        return s.to_string();
    }
    let mut end = MAX_BODY_LOG;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated {} bytes]", &s[..end], s.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_bodies() {
        let mut base = json!({"a": 1, "b": {"c": 2, "d": 3}, "e": [1]});
        deep_merge(
            &mut base,
            &json!({"a": null, "b": {"c": null, "x": {"y": null, "z": 1}}, "e": [2]}),
        );
        assert_eq!(base, json!({"b": {"d": 3, "x": {"z": 1}}, "e": [2]}));
        let mut s = json!("raw");
        deep_merge(&mut s, &json!({"a": 1}));
        assert_eq!(s, json!({"a": 1}));
    }

    #[test]
    fn encodes() {
        assert_eq!(pct_encode("a b/c&d", false), "a%20b%2Fc%26d");
        assert_eq!(pct_encode("a b/c", true), "a%20b/c");
    }

    #[test]
    fn truncates_on_boundary() {
        let s = "あ".repeat(MAX_BODY_LOG);
        let t = truncate_log(&s);
        assert!(t.contains("truncated"));
    }
}
