//! Data model of the endpoint YAML files (`api/**/*.yaml`) and the project
//! config file (`ajiro-api.yaml`).
//!
//! Field order in the structs is the order used when serializing, which keeps
//! generated files stable and diff-friendly.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 1;
pub const CONFIG_FILE: &str = "ajiro-api.yaml";

// ---------------------------------------------------------------------------
// Endpoint file
// ---------------------------------------------------------------------------

/// One endpoint = one YAML file: `api/<resource>/<method>-<path>.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EndpointDef {
    pub version: u32,
    /// Stable identifier, e.g. `users.create`. Unique within a project.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Ajiro task id (Phase 3), e.g. `TSK-012`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Endpoint ids whose tests must run before this one (e.g. `auth.login`
    /// that captures a token).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    pub request: RequestSpec,
    /// Written by `scan` only. Humans and AI should not edit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceInfo>,
    /// Written by humans and AI. `scan` never touches it.
    #[serde(default)]
    pub tests: Vec<TestCase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock: Option<MockSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RequestSpec {
    /// Upper-case HTTP method.
    pub method: String,
    /// OpenAPI-style path with `{param}` placeholders, e.g. `/users/{id}`.
    pub path: String,
    /// Values for `{param}` placeholders in `path`. Templates allowed.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub params: IndexMap<String, Value>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub query: IndexMap<String, Value>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    /// JSON body (any YAML value). Strings inside are templated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Overrides applied on top of the endpoint `request` for one test case.
/// Maps (`params`, `query`, `headers`) are merged key by key; `body` objects
/// are deep-merged (a `null` value removes the key); any other body type
/// replaces the base body.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RequestOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub params: IndexMap<String, Value>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub query: IndexMap<String, Value>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceInfo {
    pub framework: String,
    /// Path relative to the project root, `/`-separated.
    pub file: String,
    pub line: u32,
    /// Handler / function name when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestOverride>,
    #[serde(default)]
    pub expect: Expect,
    /// Variables to capture from the response: `name: <JSONPath>`,
    /// `name: header:<Header-Name>`, `name: cookie:<cookie-name>` (from
    /// `Set-Cookie`), `name: status` or `name: body`.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub capture: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub skip: bool,
    /// Only run in these environments (empty = all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only_env: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusExpect>,
    /// Header name (case-insensitive) -> expected value. A value starting and
    /// ending with `/` is treated as a regex (e.g. `/^application\/json/`).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyExpect>,
    /// JSONPaths excluded from `equals` and `snapshot` comparison
    /// (volatile values such as ids and timestamps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    /// Maximum response time in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum StatusExpect {
    Code(u16),
    AnyOf(Vec<u16>),
}

impl StatusExpect {
    pub fn matches(&self, status: u16) -> bool {
        match self {
            StatusExpect::Code(c) => *c == status,
            StatusExpect::AnyOf(v) => v.contains(&status),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BodyExpect {
    /// Exact match against the whole JSON body (after removing `ignore` paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
    /// JSON Schema: a path relative to the endpoint YAML file, or an inline schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<SchemaRef>,
    /// JSONPath -> expected value (partial match). The expected value may be a
    /// literal or a matcher object with exactly one key among:
    /// `$exists: bool`, `$type: string|number|integer|boolean|array|object|null`,
    /// `$regex: str`, `$contains: any`, `$len: n`, `$gt: n`, `$gte: n`,
    /// `$lt: n`, `$lte: n`, `$ne: any`, `$oneOf: [..]`.
    #[serde(rename = "match", default, skip_serializing_if = "IndexMap::is_empty")]
    pub match_: IndexMap<String, Value>,
    /// Compare with a stored snapshot (`api/__snapshots__/<id>/<test>.json`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub snapshot: bool,
    /// Substring that must appear in a non-JSON (text) body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SchemaRef {
    Path(String),
    Inline(Value),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MockSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<MockResponse>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub scenarios: IndexMap<String, MockResponse>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MockResponse {
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// Artificial latency in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
}

fn default_status() -> u16 {
    200
}

// ---------------------------------------------------------------------------
// Project config: ajiro-api.yaml
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub version: u32,
    /// `nextjs` | `hono` | `fastapi` | `express` | `go-nethttp` | `go-chi` | `go-echo`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// Source directories to scan, relative to the project root.
    #[serde(default = "default_sources")]
    pub sources: Vec<String>,
    /// Glob-ish substrings of paths to exclude from scanning.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Directory holding endpoint YAML files.
    #[serde(default = "default_api_dir")]
    pub api_dir: String,
    /// Dotenv file with secrets, referenced as `{{ env.NAME }}`.
    #[serde(default = "default_env_file")]
    pub env_file: String,
    #[serde(default = "default_env_name")]
    pub default_env: String,
    #[serde(default)]
    pub environments: IndexMap<String, Environment>,
    /// Extra header / JSON key names to mask in logs (case-insensitive).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        let mut environments = IndexMap::new();
        environments.insert(
            "local".to_string(),
            Environment {
                base_url: "http://localhost:3000".into(),
                ..Default::default()
            },
        );
        ProjectConfig {
            version: SCHEMA_VERSION,
            framework: None,
            sources: default_sources(),
            exclude: vec![],
            api_dir: default_api_dir(),
            env_file: default_env_file(),
            default_env: default_env_name(),
            environments,
            redact: vec![],
            timeout_ms: default_timeout(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub base_url: String,
    /// Variables available as `{{ name }}`. Templates allowed (e.g. `{{ env.X }}`).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub variables: IndexMap<String, Value>,
    /// Headers sent with every request in this environment. Templates are
    /// rendered per request, so they can use values captured by `setup`
    /// endpoints (e.g. `Cookie: "session={{ session }}"`).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub headers: IndexMap<String, String>,
    /// Endpoint ids run before every other endpoint (e.g. a login that
    /// captures a session cookie). They always run, even when a filter is
    /// given, and a failure stops the run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<String>,
    /// Environment-specific dotenv file (overrides the project one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
}

fn default_sources() -> Vec<String> {
    vec![".".into()]
}
fn default_api_dir() -> String {
    "api".into()
}
fn default_env_file() -> String {
    ".env".into()
}
fn default_env_name() -> String {
    "local".into()
}
fn default_timeout() -> u64 {
    10_000
}

/// Supported frameworks (value of `framework` / `source.framework`).
pub const FRAMEWORKS: &[&str] = &[
    "nextjs",
    "hono",
    "fastapi",
    "express",
    "go-nethttp",
    "go-chi",
    "go-echo",
];
