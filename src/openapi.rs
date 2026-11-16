//! OpenAPI 3.x import/export. (OWNER: integrations agent)
use crate::cli::OpenapiCommand;
use anyhow::{bail, Result};
use std::path::Path;

pub fn run_cli(_dir: &Path, _cmd: &OpenapiCommand) -> Result<i32> {
    bail!("openapi: not implemented yet")
}
