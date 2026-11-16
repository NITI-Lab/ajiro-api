//! Hono scanner: `new Hono()` / `OpenAPIHono` instances, `.get/.post/...`,
//! `.on()`, `.route()` mounts (across files), `.basePath()` and
//! `app.openapi(createRoute({...}))`.
//!
//! `.all()` is emitted as a single GET route (with a warning). Apps
//! registered through helper factories (`createFactory`) are not detected.

use super::ts_common::{scan_graphs, Flavor};
use super::Route;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn scan(root: &Path, files: &[PathBuf]) -> Result<Vec<Route>> {
    Ok(scan_with_warnings(root, files)?.0)
}

pub fn scan_with_warnings(root: &Path, files: &[PathBuf]) -> Result<(Vec<Route>, Vec<String>)> {
    scan_graphs(Flavor::Hono, root, files)
}
