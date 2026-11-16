//! Postman / Bruno importers. (OWNER: integrations agent)
use crate::cli::ImportCommand;
use anyhow::{bail, Result};
use std::path::Path;

pub fn run_cli(_dir: &Path, _cmd: &ImportCommand) -> Result<i32> {
    bail!("import: not implemented yet")
}
