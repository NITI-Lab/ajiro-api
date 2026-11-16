//! (OWNER: scan agent — TypeScript frameworks)
use super::Route;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn scan(_root: &Path, _files: &[PathBuf]) -> Result<Vec<Route>> {
    Ok(vec![])
}
