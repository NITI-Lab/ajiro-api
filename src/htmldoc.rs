//! Single-file HTML API reference. (OWNER: integrations agent)
//!
//! The generated page is fully self-contained (inline CSS/JS, no external
//! requests) and works from `file://`. All dynamic text is HTML-escaped.

use crate::cli::DocsArgs;
use crate::config::Project;
use crate::model::{MockResponse, StatusExpect};
use crate::naming;
use crate::project::{load_endpoints, LoadedEndpoint};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fmt::Write as _;
use std::path::Path;

const REPO_URL: &str = "https://github.com/niti-lab/ajiro-api";

pub fn run_cli(dir: &Path, args: &DocsArgs) -> Result<i32> {
    let project = Project::discover_or_default(dir)?;
    let html = generate(&project, args.title.as_deref(), args.env.as_deref())?;
    if let Some(parent) = args.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&args.out, html)
        .with_context(|| format!("cannot write {}", args.out.display()))?;
    eprintln!("wrote {}", args.out.display());
    Ok(crate::exit::OK)
}

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

fn base_url(project: &Project, env: Option<&str>) -> Result<String> {
    let cfg = &project.config;
    if let Some(name) = env {
        return match cfg.environments.get(name) {
            Some(e) => Ok(e.base_url.clone()),
            None => bail!(
                "unknown environment `{}` (available: {})",
                name,
                cfg.environments
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }
    Ok(cfg
        .environments
        .get(&cfg.default_env)
        .or_else(|| cfg.environments.values().next())
        .map(|e| e.base_url.clone())
        .unwrap_or_else(|| "http://localhost:3000".into()))
}

fn is_secret_header(name: &str, value: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("x-api-key")
        || name.eq_ignore_ascii_case("cookie")
        || value.contains("{{ env.")
        || value.contains("{{env.")
}

fn mask_header(name: &str, value: &str) -> String {
    if is_secret_header(name, value) {
        if name.eq_ignore_ascii_case("authorization")
            && value.trim().to_ascii_lowercase().starts_with("bearer")
        {
            "Bearer <token>".into()
        } else {
            "<token>".into()
        }
    } else {
        value.to_string()
    }
}

fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            let _ = write!(o, "%{:02X}", b);
        }
    }
    o
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn url_for(ep: &LoadedEndpoint, base: &str) -> String {
    let mut path = ep.def.request.path.clone();
    for name in naming::path_params(&path) {
        if let Some(v) = ep.def.request.params.get(&name) {
            let s = scalar(v);
            if !s.is_empty() && !s.contains("{{") {
                path = path.replace(&format!("{{{}}}", name), &pct(&s));
            }
        }
    }
    let q: Vec<String> = ep
        .def
        .request
        .query
        .iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| format!("{}={}", pct(k), pct(&scalar(v))))
        .collect();
    let mut url = format!("{}{}", base.trim_end_matches('/'), path);
    if !q.is_empty() {
        url.push('?');
        url.push_str(&q.join("&"));
    }
    url
}

fn curl_for(ep: &LoadedEndpoint, base: &str) -> String {
    let r = &ep.def.request;
    let mut parts = vec![format!(
        "curl -X {} {}",
        r.method.to_ascii_uppercase(),
        sh_quote(&url_for(ep, base))
    )];
    let mut has_ct = false;
    for (k, v) in &r.headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_ct = true;
        }
        parts.push(format!(
            "-H {}",
            sh_quote(&format!("{}: {}", k, mask_header(k, v)))
        ));
    }
    if let Some(b) = &r.body {
        let (data, ct) = match b {
            Value::String(s) => (s.clone(), "text/plain"),
            other => (
                serde_json::to_string(other).unwrap_or_default(),
                "application/json",
            ),
        };
        if !has_ct {
            parts.push(format!("-H {}", sh_quote(&format!("Content-Type: {}", ct))));
        }
        parts.push(format!("-d {}", sh_quote(&data)));
    }
    parts.join(" \\\n  ")
}

fn pretty(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn code_block(out: &mut String, text: &str) {
    let _ = write!(
        out,
        "<div class=\"code\"><button class=\"copy\" type=\"button\">Copy</button><pre>{}</pre></div>",
        esc(text)
    );
}

fn status_class(code: u16) -> &'static str {
    match code / 100 {
        2 => "s2",
        3 => "s3",
        4 => "s4",
        5 => "s5",
        _ => "s0",
    }
}

fn response_block(out: &mut String, label: &str, r: &MockResponse) {
    let _ = write!(
        out,
        "<div class=\"resp\"><div class=\"resp-h\"><span class=\"st {}\">{}</span><span class=\"muted\">{}</span></div>",
        status_class(r.status),
        r.status,
        esc(label)
    );
    if !r.headers.is_empty() {
        let h: Vec<String> = r
            .headers
            .iter()
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect();
        code_block(out, &h.join("\n"));
    }
    if let Some(b) = &r.body {
        code_block(out, &pretty(b));
    }
    out.push_str("</div>");
}

fn render_endpoint(out: &mut String, ep: &LoadedEndpoint, base: &str, anchor: &str) {
    let d = &ep.def;
    let method = d.request.method.to_ascii_uppercase();
    let search = format!(
        "{} {} {} {} {}",
        method,
        d.request.path,
        d.id,
        d.summary.clone().unwrap_or_default(),
        d.tags.join(" ")
    )
    .to_lowercase();
    let _ = write!(
        out,
        "<section class=\"ep\" id=\"{}\" data-s=\"{}\"><h2><span class=\"m {}\">{}</span><code class=\"path\">{}</code></h2>",
        esc(anchor),
        esc(&search),
        esc(&method.to_lowercase()),
        esc(&method),
        esc(&d.request.path)
    );
    if let Some(s) = &d.summary {
        let _ = write!(out, "<p class=\"summary\">{}</p>", esc(s));
    }
    if let Some(s) = &d.description {
        let _ = write!(out, "<p class=\"desc\">{}</p>", esc(s));
    }
    let _ = write!(
        out,
        "<div class=\"meta\"><span>id <code>{}</code></span>",
        esc(&d.id)
    );
    if let Some(t) = &d.task {
        let _ = write!(out, "<span>task <code>{}</code></span>", esc(t));
    }
    if let Some(s) = &d.source {
        let _ = write!(
            out,
            "<span>source <code>{}:{}</code></span>",
            esc(&s.file),
            s.line
        );
    }
    let _ = write!(out, "<span>file <code>{}</code></span>", esc(&ep.rel_path));
    for t in &d.tags {
        let _ = write!(out, "<span class=\"tag\">{}</span>", esc(t));
    }
    out.push_str("</div>");

    // request
    out.push_str("<h3>Request</h3>");
    let mut req = format!("{} {}", method, d.request.path);
    for (k, v) in &d.request.headers {
        let _ = write!(req, "\n{}: {}", k, mask_header(k, v));
    }
    code_block(out, &req);
    if !d.request.params.is_empty() {
        out.push_str("<h4>Path parameters</h4><table>");
        for (k, v) in &d.request.params {
            let _ = write!(
                out,
                "<tr><td><code>{}</code></td><td><code>{}</code></td></tr>",
                esc(k),
                esc(&scalar(v))
            );
        }
        out.push_str("</table>");
    }
    if !d.request.query.is_empty() {
        out.push_str("<h4>Query</h4><table>");
        for (k, v) in &d.request.query {
            let _ = write!(
                out,
                "<tr><td><code>{}</code></td><td><code>{}</code></td></tr>",
                esc(k),
                esc(&scalar(v))
            );
        }
        out.push_str("</table>");
    }
    if let Some(b) = &d.request.body {
        out.push_str("<h4>Body</h4>");
        code_block(out, &pretty(b));
    }
    out.push_str("<h4>curl</h4>");
    code_block(out, &curl_for(ep, base));

    // responses
    let mut any = false;
    if let Some(m) = &d.mock {
        if m.default.is_some() || !m.scenarios.is_empty() {
            out.push_str("<h3>Sample responses</h3>");
            any = true;
        }
        if let Some(r) = &m.default {
            response_block(out, "default", r);
        }
        for (name, r) in &m.scenarios {
            response_block(out, &format!("scenario: {}", name), r);
        }
    }
    let expects: Vec<_> = d
        .tests
        .iter()
        .filter(|t| t.expect.status.is_some())
        .collect();
    if !expects.is_empty() {
        out.push_str("<h3>Expected by tests</h3><table>");
        for t in expects {
            let st = match t.expect.status.as_ref().unwrap() {
                StatusExpect::Code(c) => {
                    format!("<span class=\"st {}\">{}</span>", status_class(*c), c)
                }
                StatusExpect::AnyOf(v) => v
                    .iter()
                    .map(|c| format!("<span class=\"st {}\">{}</span>", status_class(*c), c))
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            let _ = write!(out, "<tr><td>{}</td><td>{}</td></tr>", esc(&t.name), st);
        }
        out.push_str("</table>");
        any = true;
    }
    let _ = any;
    out.push_str("</section>");
}

fn resource_of(ep: &LoadedEndpoint) -> String {
    let parts: Vec<&str> = ep.rel_path.split('/').collect();
    if parts.len() >= 2 {
        let p = parts[parts.len() - 2];
        // the api dir itself (flat layout) is not a resource name
        if p != "api" {
            return p.to_string();
        }
    }
    naming::resource_of(&ep.def.request.path)
}

/// Build the whole HTML document.
pub fn generate(project: &Project, title: Option<&str>, env: Option<&str>) -> Result<String> {
    let base = base_url(project, env)?;
    let (eps, _errs) = load_endpoints(project);
    let title = title.map(str::to_string).unwrap_or_else(|| {
        let n = project
            .root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "API".into());
        format!("{} API", n)
    });

    let mut groups: Vec<(String, Vec<(usize, String)>)> = Vec::new();
    for (i, ep) in eps.iter().enumerate() {
        let r = resource_of(ep);
        let anchor = format!("ep-{}-{}", naming::slug(&ep.def.id), i);
        match groups.iter_mut().find(|(n, _)| *n == r) {
            Some((_, v)) => v.push((i, anchor)),
            None => groups.push((r, vec![(i, anchor)])),
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    let mut o = String::new();
    let _ = write!(
        o,
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><style>{}</style></head><body>",
        esc(&title),
        CSS
    );
    let _ = write!(
        o,
        "<aside id=\"side\"><div class=\"brand\">{}</div><input id=\"q\" type=\"search\" placeholder=\"Search endpoints\" autocomplete=\"off\"><nav>",
        esc(&title)
    );
    for (name, items) in &groups {
        let _ = write!(o, "<div class=\"grp\"><h3>{}</h3>", esc(name));
        for (i, anchor) in items {
            let d = &eps[*i].def;
            let method = d.request.method.to_ascii_uppercase();
            let search = format!(
                "{} {} {} {}",
                method,
                d.request.path,
                d.id,
                d.summary.clone().unwrap_or_default()
            )
            .to_lowercase();
            let _ = write!(
                o,
                "<a class=\"item\" href=\"#{}\" data-s=\"{}\"><span class=\"m {}\">{}</span><span class=\"p\">{}</span></a>",
                esc(anchor),
                esc(&search),
                esc(&method.to_lowercase()),
                esc(&method),
                esc(&d.request.path)
            );
        }
        o.push_str("</div>");
    }
    o.push_str("</nav></aside><main>");
    let _ = write!(
        o,
        "<header><h1>{}</h1><p class=\"muted\">{} endpoint{} &middot; base URL <code>{}</code></p></header>",
        esc(&title),
        eps.len(),
        if eps.len() == 1 { "" } else { "s" },
        esc(&base)
    );
    if eps.is_empty() {
        o.push_str(
            "<p class=\"muted\">No endpoints found. Run <code>ajiro-api scan</code> first.</p>",
        );
    }
    for (_, items) in &groups {
        for (i, anchor) in items {
            render_endpoint(&mut o, &eps[*i], &base, anchor);
        }
    }
    o.push_str("<p id=\"none\" class=\"muted\" hidden>No matching endpoints.</p></main>");
    let _ = writeln!(
        o,
        "<a class=\"credit\" href=\"{}\" target=\"_blank\" rel=\"noopener\">Generated by Ajiro API</a><script>{}</script></body></html>",
        REPO_URL, JS
    );
    Ok(o)
}

const CSS: &str = r#"
:root{--bg:#fbfbfa;--panel:#ffffff;--fg:#1c1d1f;--muted:#6b7077;--line:#e4e5e7;--code:#f3f4f5;--accent:#3b5bdb;
--get:#1a7f4b;--post:#2563c9;--put:#b7791f;--patch:#8a5cc2;--delete:#c53030;--other:#6b7077}
@media (prefers-color-scheme:dark){:root{--bg:#141517;--panel:#1b1c1f;--fg:#e6e7e9;--muted:#9aa0a8;--line:#2b2d31;--code:#222428;--accent:#8ea2ff;
--get:#4cc38a;--post:#6aa3ff;--put:#e0b252;--patch:#b794f4;--delete:#ff7b7b;--other:#9aa0a8}}
*{box-sizing:border-box}[hidden]{display:none!important}html{scroll-behavior:smooth}
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.6 -apple-system,BlinkMacSystemFont,"Segoe UI","Hiragino Sans","Noto Sans JP",Meiryo,sans-serif;display:flex;min-height:100vh}
code,pre{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:13px}
code{background:var(--code);padding:1px 5px;border-radius:4px}
#side{position:sticky;top:0;height:100vh;width:300px;flex:none;overflow:auto;background:var(--panel);border-right:1px solid var(--line);padding:18px 14px}
.brand{font-weight:700;font-size:16px;margin-bottom:12px}
#q{width:100%;padding:8px 10px;border:1px solid var(--line);border-radius:8px;background:var(--bg);color:var(--fg);margin-bottom:14px;font:inherit}
.grp h3{font-size:11px;text-transform:uppercase;letter-spacing:.08em;color:var(--muted);margin:14px 6px 4px}
.item{display:flex;gap:8px;align-items:center;padding:5px 6px;border-radius:6px;color:var(--fg);text-decoration:none;font-size:13px}
.item:hover{background:var(--code)}
.item .p{font-family:ui-monospace,Menlo,monospace;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.m{display:inline-block;min-width:52px;text-align:center;font:700 11px/1 ui-monospace,Menlo,monospace;padding:4px 6px;border-radius:5px;color:#fff;background:var(--other)}
.m.get{background:var(--get)}.m.post{background:var(--post)}.m.put{background:var(--put)}.m.patch{background:var(--patch)}.m.delete{background:var(--delete)}
main{flex:1;min-width:0;max-width:920px;padding:32px 36px 90px}
header h1{margin:0 0 4px;font-size:28px}
.muted{color:var(--muted)}
.ep{background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:20px 24px;margin:22px 0;scroll-margin-top:16px}
.ep h2{display:flex;align-items:center;gap:12px;margin:0 0 6px;font-size:18px;flex-wrap:wrap}
.ep h2 .path{background:none;padding:0;font-size:16px;word-break:break-all}
.ep h3{margin:22px 0 8px;font-size:13px;text-transform:uppercase;letter-spacing:.06em;color:var(--muted)}
.ep h4{margin:14px 0 6px;font-size:13px}
.summary{font-weight:600;margin:4px 0}.desc{margin:4px 0;white-space:pre-wrap}
.meta{display:flex;flex-wrap:wrap;gap:6px 16px;color:var(--muted);font-size:13px;margin-top:6px}
.tag{border:1px solid var(--line);border-radius:999px;padding:0 8px}
.code{position:relative;margin:6px 0}
.code pre{margin:0;background:var(--code);border-radius:8px;padding:12px 14px;overflow:auto;white-space:pre}
.copy{position:absolute;top:6px;right:6px;border:1px solid var(--line);background:var(--panel);color:var(--muted);border-radius:6px;padding:2px 8px;font-size:12px;cursor:pointer}
.copy:hover{color:var(--fg)}
table{border-collapse:collapse;width:100%;font-size:13px}td{padding:5px 8px;border-bottom:1px solid var(--line);vertical-align:top}
.resp{margin:8px 0}.resp-h{display:flex;gap:10px;align-items:center}
.st{font:700 12px ui-monospace,Menlo,monospace;padding:2px 7px;border-radius:5px;background:var(--code)}
.st.s2{color:var(--get)}.st.s3{color:var(--put)}.st.s4,.st.s5{color:var(--delete)}
.credit{position:fixed;right:14px;bottom:12px;background:var(--panel);border:1px solid var(--line);border-radius:999px;padding:5px 12px;font-size:12px;color:var(--muted);text-decoration:none;box-shadow:0 2px 8px rgba(0,0,0,.08)}
.credit:hover{color:var(--accent)}
@media (max-width:820px){body{display:block}#side{position:static;height:auto;width:auto;max-height:42vh;border-right:0;border-bottom:1px solid var(--line)}main{padding:18px 14px 80px}}
"#;

const JS: &str = r#"
(function(){
  function copyText(t,b){
    var done=function(){var o=b.textContent;b.textContent='Copied';setTimeout(function(){b.textContent=o},1200)};
    if(navigator.clipboard&&window.isSecureContext){navigator.clipboard.writeText(t).then(done,function(){fb(t,done)})}else{fb(t,done)}
  }
  function fb(t,done){var a=document.createElement('textarea');a.value=t;a.style.position='fixed';a.style.opacity='0';document.body.appendChild(a);a.select();try{document.execCommand('copy')}catch(e){}document.body.removeChild(a);done()}
  document.querySelectorAll('button.copy').forEach(function(b){b.addEventListener('click',function(){copyText(b.parentElement.querySelector('pre').textContent,b)})});
  var q=document.getElementById('q');
  q.addEventListener('input',function(){
    var s=q.value.trim().toLowerCase(),shown=0;
    document.querySelectorAll('.ep').forEach(function(e){var ok=!s||e.dataset.s.indexOf(s)>=0;e.hidden=!ok;if(ok)shown++});
    document.querySelectorAll('.item').forEach(function(e){e.hidden=!!s&&e.dataset.s.indexOf(s)<0});
    document.querySelectorAll('.grp').forEach(function(g){g.hidden=!g.querySelector('.item:not([hidden])')});
    document.getElementById('none').hidden=shown>0||!s;
  });
})();
"#;
