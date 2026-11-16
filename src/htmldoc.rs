//! Single-file HTML API reference. (OWNER: integrations agent)
use crate::cli::DocsArgs;
use anyhow::{bail, Result};
use std::path::Path;

pub fn run_cli(_dir: &Path, _args: &DocsArgs) -> Result<i32> {
    bail!("docs: not implemented yet")
}
