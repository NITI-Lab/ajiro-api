//! Ajiro API — API definitions, regression tests and mocks for AI coding agents.
//!
//! Module ownership (see CONTRIBUTING.md):
//! - `model`, `config`, `project`, `naming`, `report`, `cli`, `telemetry`, `schema`: core
//! - `init`, `scan`: route discovery
//! - `runner`: `test` command (HTTP execution, assertions, reporters)
//! - `mcp`, `mock`, `openapi`, `importers`, `htmldoc`: integrations

pub mod cli;
pub mod config;
pub mod htmldoc;
pub mod importers;
pub mod init;
pub mod mcp;
pub mod mock;
pub mod model;
pub mod naming;
pub mod openapi;
pub mod project;
pub mod project_cli;
pub mod report;
pub mod runner;
pub mod scan;
pub mod schema;
pub mod telemetry;

/// Process exit codes used by every command.
pub mod exit {
    /// Everything passed.
    pub const OK: i32 = 0;
    /// Tests failed / `scan --check` found drift / validation errors.
    pub const FAILURE: i32 = 1;
    /// Usage, config or I/O error (the tool itself could not do its job).
    pub const ERROR: i32 = 2;
}
