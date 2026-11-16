//! `test`: run endpoint requests and check responses. (OWNER: runner agent)

use crate::cli::TestArgs;
use crate::config::Project;
use crate::report::{TestOptions, TestReport};
use anyhow::{bail, Result};
use std::path::Path;

pub fn run_tests(_project: &Project, _opts: &TestOptions) -> Result<TestReport> {
    bail!("test: not implemented yet")
}

pub fn run_cli(_dir: &Path, _args: &TestArgs) -> Result<i32> {
    bail!("test: not implemented yet")
}
