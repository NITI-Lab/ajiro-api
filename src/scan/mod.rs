//! `scan`: static route analysis -> endpoint YAML files. (OWNER: scan agent)
//!
//! Each framework module exposes `pub fn scan(project_root: &Path, files: &[PathBuf]) -> Result<Vec<Route>>`.

pub mod express;
pub mod fastapi;
pub mod go;
pub mod hono;
pub mod nextjs;

use crate::cli::ScanArgs;
use crate::config::Project;
use crate::report::{ScanOptions, ScanReport};
use anyhow::{bail, Result};
use std::path::Path;

/// A route found in the source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Upper-case HTTP method.
    pub method: String,
    /// Normalized OpenAPI-style path (see `naming::normalize_path`).
    pub path: String,
    /// Path relative to the project root, `/`-separated.
    pub file: String,
    /// 1-based line of the route registration / handler.
    pub line: u32,
    pub handler: Option<String>,
    /// Query parameter names when statically known (FastAPI signatures, ...).
    pub query_params: Vec<String>,
    /// `framework` value written into `source.framework`.
    pub framework: String,
}

pub fn scan_project(_project: &Project, _opts: &ScanOptions) -> Result<ScanReport> {
    bail!("scan: not implemented yet")
}

pub fn run_cli(_dir: &Path, _args: &ScanArgs) -> Result<i32> {
    bail!("scan: not implemented yet")
}
