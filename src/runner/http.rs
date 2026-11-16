//! Thin wrapper around `ureq` 3: no redirects, no status-as-error, bodies
//! read fully.

use serde_json::Value;
use std::io::Read;
use std::time::{Duration, Instant};

pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub text: String,
    /// Parsed body when it looks like JSON.
    pub json: Option<Value>,
    pub elapsed_ms: u64,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug)]
pub enum SendError {
    ConnectionRefused(String),
    Other(String),
}

pub fn is_loopback(base_url: &str) -> bool {
    let rest = base_url.split("://").nth(1).unwrap_or(base_url);
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = if host.starts_with('[') {
        host.split(']')
            .next()
            .unwrap_or(host)
            .trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or(host)
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
}

pub fn make_agent(timeout_ms: u64, base_url: &str) -> ureq::Agent {
    let mut b = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .user_agent(concat!("ajiro-api/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(Duration::from_millis(timeout_ms.max(1))));
    if is_loopback(base_url) {
        b = b.proxy(None);
    }
    b.build().into()
}

pub fn send(agent: &ureq::Agent, req: &HttpRequest) -> Result<Response, SendError> {
    let mut builder = ureq::http::Request::builder()
        .method(req.method.as_str())
        .uri(req.url.as_str());
    for (k, v) in &req.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let request = builder
        .body(req.body.clone().unwrap_or_default())
        .map_err(|e| SendError::Other(format!("invalid request: {}", e)))?;

    let start = Instant::now();
    let mut resp = agent.run(request).map_err(|e| {
        let msg = e.to_string();
        let refused = match &e {
            ureq::Error::Io(io) => io.kind() == std::io::ErrorKind::ConnectionRefused,
            _ => false,
        } || msg.to_lowercase().contains("refused");
        if refused {
            SendError::ConnectionRefused(msg)
        } else {
            SendError::Other(msg)
        }
    })?;

    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let mut bytes = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(32 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| SendError::Other(format!("failed to read response body: {}", e)))?;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let text = String::from_utf8_lossy(&bytes).into_owned();
    let is_json_ct = headers
        .iter()
        .any(|(k, v)| k == "content-type" && v.to_ascii_lowercase().contains("json"));
    let trimmed = text.trim_start();
    let json = if !trimmed.is_empty()
        && (is_json_ct || trimmed.starts_with('{') || trimmed.starts_with('['))
    {
        serde_json::from_str(&text).ok()
    } else {
        None
    };
    Ok(Response {
        status,
        headers,
        text,
        json,
        elapsed_ms,
    })
}
