//! Report types shared by the CLI (`--json`), the MCP server and reporters.
//! These are part of the public, machine-readable contract: add fields, but
//! do not rename or remove them without bumping `REPORT_VERSION`.

use crate::model::SourceInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const REPORT_VERSION: u32 = 1;

// ----------------------------------------------------------------- scan ---

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Force a framework instead of auto-detection / config.
    pub framework: Option<String>,
    /// Directories or files to scan (default: config `sources`).
    pub paths: Vec<PathBuf>,
    /// Do not write files.
    pub dry_run: bool,
    /// Do not write files; report drift (used by CI: exit 1 if anything would change).
    pub check: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanReport {
    pub report_version: u32,
    pub framework: String,
    pub endpoints: Vec<ScanEntry>,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    /// YAML files (relative paths) whose route no longer exists in the code.
    /// They are never deleted automatically.
    pub stale: Vec<String>,
    pub warnings: Vec<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScanStatus {
    Created,
    Updated,
    Unchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanEntry {
    pub id: String,
    pub method: String,
    pub path: String,
    /// Endpoint YAML file, relative to the project root.
    pub file: String,
    pub source: SourceInfo,
    pub status: ScanStatus,
}

// ----------------------------------------------------------------- test ---

#[derive(Debug, Clone, Default)]
pub struct TestOptions {
    /// Environment name (default: config `default_env`).
    pub env: Option<String>,
    /// Only run endpoints/tests whose id, file or test name contains this.
    pub filter: Option<String>,
    /// Overwrite stored snapshots with the actual responses.
    pub update_snapshots: bool,
    /// Stop at the first failure.
    pub bail: bool,
    /// Override the environment `base_url`.
    pub base_url: Option<String>,
    /// Override config `timeout_ms`.
    pub timeout_ms: Option<u64>,
    /// Also run the implicit smoke test for endpoints whose method may change
    /// data (POST, PUT, PATCH, DELETE, ...). Off by default.
    pub smoke_unsafe: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestReport {
    pub report_version: u32,
    pub env: String,
    pub base_url: String,
    pub ok: bool,
    pub summary: TestSummary,
    pub results: Vec<TestResult>,
    /// Endpoint files that could not be loaded.
    pub load_errors: Vec<crate::project::LoadError>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Tests that could not run (connection refused, template error, ...).
    pub errors: usize,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub endpoint_id: String,
    /// Endpoint YAML file, relative to the project root.
    pub file: String,
    pub test: String,
    /// Index of the test in the file's `tests` array.
    pub test_index: usize,
    pub status: TestStatus,
    pub duration_ms: u64,
    /// Request as sent (secrets masked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<RequestLog>,
    /// Response as received (secrets masked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseLog>,
    pub failures: Vec<Failure>,
    /// Error message when `status == error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Where the handler lives in the code, from the endpoint `source`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestLog {
    pub method: String,
    pub url: String,
    pub headers: indexmap::IndexMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponseLog {
    pub status: u16,
    pub headers: indexmap::IndexMap<String, String>,
    /// Parsed JSON, or a JSON string for non-JSON bodies (truncated to 64 KiB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    /// What was checked: `status`, `header`, `body.equals`, `body.schema`,
    /// `body.match`, `body.snapshot`, `body.contains`, `max_ms`, `capture`.
    pub check: String,
    /// JSONPath / header name / schema pointer the failure is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub expected: Value,
    pub actual: Value,
    /// Human readable one-line explanation.
    pub message: String,
}
