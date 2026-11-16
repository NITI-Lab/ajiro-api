//! Mock server. (OWNER: integrations agent)
use crate::cli::MockArgs;
use anyhow::{bail, Result};
use std::path::Path;

pub fn run_cli(_dir: &Path, _args: &MockArgs) -> Result<i32> {
    bail!("mock: not implemented yet")
}
