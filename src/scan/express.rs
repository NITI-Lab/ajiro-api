//! Express scanner: `express()` / `Router()` instances, `.get/.post/...`,
//! `.route('/x').get().post()`, `.use('/prefix', router)` mounts across files
//! (ESM imports and CommonJS `require` / `module.exports`).
//!
//! `.all()` is emitted as a single GET route (with a warning). Regex paths
//! and non-literal paths are skipped with a warning.

use super::ts_common::{scan_graphs, Flavor};
use super::Route;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn scan(root: &Path, files: &[PathBuf]) -> Result<Vec<Route>> {
    Ok(scan_with_warnings(root, files)?.0)
}

pub fn scan_with_warnings(root: &Path, files: &[PathBuf]) -> Result<(Vec<Route>, Vec<String>)> {
    scan_graphs(Flavor::Express, root, files)
}
