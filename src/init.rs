//! `init`: create ajiro-api.yaml + api/, detect the framework. (OWNER: scan agent)

use crate::cli::InitArgs;
use anyhow::{bail, Result};
use std::path::Path;

/// Detect the framework used in `root` (package.json deps, pyproject/requirements, go.mod).
pub fn detect_framework(_root: &Path) -> Option<String> {
    None
}

pub fn run_cli(_dir: &Path, _args: &InitArgs) -> Result<i32> {
    bail!("init: not implemented yet")
}
