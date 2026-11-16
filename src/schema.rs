//! JSON Schemas of the file formats, embedded in the binary.
//! The source of truth is `schema/*.schema.json` (published for editors).

use crate::cli::SchemaArgs;
use anyhow::{bail, Result};

pub const ENDPOINT_SCHEMA: &str = include_str!("../schema/endpoint.schema.json");
pub const CONFIG_SCHEMA: &str = include_str!("../schema/config.schema.json");

pub fn run_cli(args: &SchemaArgs) -> Result<i32> {
    match args.which.as_str() {
        "endpoint" => println!("{}", ENDPOINT_SCHEMA),
        "config" => println!("{}", CONFIG_SCHEMA),
        other => bail!("unknown schema `{}` (expected endpoint or config)", other),
    }
    Ok(crate::exit::OK)
}
