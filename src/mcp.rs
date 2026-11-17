//! MCP server over stdio. (OWNER: integrations agent)
//!
//! Newline-delimited JSON-RPC 2.0 on stdin/stdout; logs go to stderr only.
//! The core is [`handle_message`], which is pure with respect to the
//! transport so it can be tested without spawning a process.

use crate::config::Project;
use crate::project::load_endpoints;
use crate::project_cli::{list_items, validate_project};
use crate::report::{ScanOptions, TestOptions};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Ajiro API keeps one YAML file per HTTP endpoint (api/<resource>/<method>-<path>.yaml). \
Typical loop: 1) `scan` after changing routes (creates/updates endpoint files; it never touches `tests:` or `mock:`), \
2) edit the YAML files to add or adjust `tests:` (use `schema` to see the format, `get_endpoint` to read one), \
3) `validate`, 4) `test` against the running server and read each failure's `expected`/`actual` and `source` location, \
5) fix the code (or the test if the behaviour change is intended) and repeat. `list` shows what is defined.";

/// State of one MCP session.
#[derive(Debug, Clone)]
pub struct McpState {
    /// Working directory the project is discovered from (on every call).
    pub dir: PathBuf,
    pub initialized: bool,
}

impl McpState {
    pub fn new(dir: &Path) -> Self {
        McpState {
            dir: dir.to_path_buf(),
            initialized: false,
        }
    }
}

pub fn run_cli(dir: &Path) -> Result<i32> {
    let mut state = McpState::new(dir);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("ajiro-api mcp: stdin error: {}", e);
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle_message(&mut state, &msg),
            Err(e) => Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {}", e),
            )),
        };
        if let Some(r) = reply {
            let mut out = stdout.lock();
            writeln!(out, "{}", r)?;
            out.flush()?;
        }
    }
    Ok(crate::exit::OK)
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Handle one incoming JSON-RPC message. Returns `None` for notifications
/// (and for batches that contain only notifications).
pub fn handle_message(state: &mut McpState, msg: &Value) -> Option<Value> {
    if let Some(batch) = msg.as_array() {
        if batch.is_empty() {
            return Some(error_response(Value::Null, -32600, "empty batch"));
        }
        let replies: Vec<Value> = batch
            .iter()
            .filter_map(|m| handle_message(state, m))
            .collect();
        return if replies.is_empty() {
            None
        } else {
            Some(Value::Array(replies))
        };
    }
    let obj = match msg.as_object() {
        Some(o) => o,
        None => return Some(error_response(Value::Null, -32600, "invalid request")),
    };
    let id = obj.get("id").cloned();
    let method = match obj.get("method").and_then(Value::as_str) {
        Some(m) => m,
        None => {
            // a response from the client, or garbage: never answer
            return id.map(|id| error_response(id, -32600, "invalid request: missing method"));
        }
    };
    let id = match id {
        Some(id) if !id.is_null() => id,
        // notifications never get responses
        _ => {
            if method == "notifications/initialized" {
                state.initialized = true;
            }
            return None;
        }
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => ok_response(id, initialize_result(&params)),
        "ping" => ok_response(id, json!({})),
        "tools/list" => ok_response(id, json!({ "tools": tool_definitions() })),
        "tools/call" => match call_tool(state, &params) {
            Ok(r) => ok_response(id, r),
            Err((code, msg)) => error_response(id, code, &msg),
        },
        other => error_response(id, -32601, &format!("method not found: {}", other)),
    })
}

fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    let version = if SUPPORTED_VERSIONS.contains(&requested) {
        requested
    } else {
        SUPPORTED_VERSIONS[0]
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "ajiro-api", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

pub fn tool_definitions() -> Value {
    json!([
        {
            "name": "scan",
            "description": "Statically analyse the backend source and create/update the per-endpoint YAML files in api/. Run it after adding, removing or changing routes. Only `request` and `source` are managed by scan; `tests`, `mock`, summaries you wrote are preserved. Returns created/updated/unchanged counts, the list of endpoints with their YAML files and source locations, and `stale` files whose route no longer exists (never deleted automatically). Use dry_run to preview.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "framework": { "type": "string", "description": "Force a framework instead of auto-detection: nextjs, hono, fastapi, express, go-nethttp, go-chi, go-echo." },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Directories or files to scan, relative to the project root (default: `sources` in ajiro-api.yaml)." },
                    "dry_run": { "type": "boolean", "description": "Report what would change without writing files." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "test",
            "description": "Run the requests defined in the endpoint YAML files against the running API and check the responses. Returns a report: summary counts and one result per test; each failure has `check`, `path`, `expected`, `actual`, a one-line `message`, and the handler `source` (file:line) so you can jump to the code. The server under test must already be running at the environment's base_url. A failing test is reported in the result (ok=false), not as a tool error.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "env": { "type": "string", "description": "Environment name from ajiro-api.yaml (default: default_env)." },
                    "filter": { "type": "string", "description": "Only run endpoints/tests whose id, file or test name contains this text, e.g. `users.create`." },
                    "update_snapshots": { "type": "boolean", "description": "Overwrite stored snapshots with the actual responses. Only use when the new behaviour is intended." },
                    "base_url": { "type": "string", "description": "Override the environment base URL, e.g. http://localhost:8080." },
                    "smoke_unsafe": { "type": "boolean", "description": "Also send the implicit smoke request to endpoints without tests whose method may change data (POST, PUT, PATCH, DELETE). Off by default; do not enable against shared or production environments." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "list",
            "description": "List all endpoints defined in the api/ directory: id, method, path, YAML file, number of tests, source location and summary. Also reports files that failed to load. Use it to find the id to pass to get_endpoint or test.filter.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "validate",
            "description": "Validate all endpoint YAML files without calling the API: YAML syntax, schema, duplicate ids, unknown depends_on, unsupported methods. Run it after editing YAML files. Returns {ok, errors:[{file,message}]}.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "get_endpoint",
            "description": "Read one endpoint definition by id (as shown by `list`). Returns the YAML text, its relative file and absolute path, so you can edit the file (for example to add tests).",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string", "description": "Endpoint id, e.g. `users.create`." } },
                "required": ["id"],
                "additionalProperties": false
            }
        },
        {
            "name": "schema",
            "description": "Return the JSON Schema of the YAML formats: `endpoint` for api/**/*.yaml files (request, tests, expect matchers, mock) or `config` for ajiro-api.yaml. Read it before writing or editing tests.",
            "inputSchema": {
                "type": "object",
                "properties": { "which": { "type": "string", "enum": ["endpoint", "config"], "description": "Which schema to return." } },
                "required": ["which"],
                "additionalProperties": false
            }
        }
    ])
}

type RpcError = (i64, String);

fn text_result(v: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default() }],
        "structuredContent": v,
    })
}

fn error_result(msg: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": msg }], "isError": true })
}

fn call_tool(state: &McpState, params: &Value) -> std::result::Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call: missing `name`".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err((-32602, "tools/call: `arguments` must be an object".into()));
    }
    if !tool_definitions()
        .as_array()
        .map(|a| a.iter().any(|t| t["name"] == name))
        .unwrap_or(false)
    {
        return Err((-32602, format!("unknown tool: {}", name)));
    }
    Ok(match run_tool(state, name, &args) {
        Ok(v) => text_result(&v),
        Err(e) => error_result(&format!("{:#}", e)),
    })
}

fn str_arg(args: &Value, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(str::to_string)
}

fn bool_arg(args: &Value, k: &str) -> bool {
    args.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn run_tool(state: &McpState, name: &str, args: &Value) -> Result<Value> {
    // `schema` does not need a project.
    if name == "schema" {
        let which = str_arg(args, "which").unwrap_or_else(|| "endpoint".into());
        let text = match which.as_str() {
            "endpoint" => crate::schema::ENDPOINT_SCHEMA,
            "config" => crate::schema::CONFIG_SCHEMA,
            other => anyhow::bail!("unknown schema `{}` (expected endpoint or config)", other),
        };
        return Ok(serde_json::from_str(text)?);
    }
    let project = Project::discover(&state.dir)?;
    match name {
        "scan" => {
            let opts = ScanOptions {
                framework: str_arg(args, "framework"),
                paths: args
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(PathBuf::from)
                            .collect()
                    })
                    .unwrap_or_default(),
                dry_run: bool_arg(args, "dry_run"),
                check: false,
            };
            Ok(serde_json::to_value(crate::scan::scan_project(
                &project, &opts,
            )?)?)
        }
        "test" => {
            let opts = TestOptions {
                env: str_arg(args, "env"),
                filter: str_arg(args, "filter"),
                update_snapshots: bool_arg(args, "update_snapshots"),
                base_url: str_arg(args, "base_url"),
                smoke_unsafe: bool_arg(args, "smoke_unsafe"),
                ..Default::default()
            };
            Ok(serde_json::to_value(crate::runner::run_tests(
                &project, &opts,
            )?)?)
        }
        "list" => {
            let (items, errs) = list_items(&project);
            Ok(json!({ "endpoints": items, "load_errors": errs }))
        }
        "validate" => {
            let errs = validate_project(&project);
            Ok(json!({ "ok": errs.is_empty(), "errors": errs }))
        }
        "get_endpoint" => {
            let id = str_arg(args, "id")
                .ok_or_else(|| anyhow::anyhow!("missing required argument `id`"))?;
            let (eps, _) = load_endpoints(&project);
            match eps.iter().find(|e| e.def.id == id) {
                Some(e) => Ok(json!({
                    "id": e.def.id,
                    "file": e.rel_path,
                    "path": e.path.to_string_lossy(),
                    "yaml": std::fs::read_to_string(&e.path)?,
                })),
                None => {
                    let known: Vec<&str> = eps.iter().map(|e| e.def.id.as_str()).take(50).collect();
                    anyhow::bail!(
                        "no endpoint with id `{}`. Known ids: {}",
                        id,
                        known.join(", ")
                    )
                }
            }
        }
        other => anyhow::bail!("unknown tool {}", other),
    }
}
