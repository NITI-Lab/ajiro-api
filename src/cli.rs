//! Command-line interface. Argument structs live here; each command's logic
//! lives in its own module and is reached through a `run_cli` function that
//! returns the process exit code (see [`crate::exit`]).

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "ajiro-api",
    version,
    about = "API definitions, regression tests and mocks for AI coding agents",
    long_about = "Ajiro API scans your backend routes into plain YAML files, runs them as \
regression tests and serves them as mocks. Designed to be driven by Claude Code / Codex: \
every command has a --json mode with stable, machine-readable output."
)]
pub struct Cli {
    /// Project directory (default: current directory; parents are searched for ajiro-api.yaml)
    #[arg(short = 'C', long = "dir", global = true, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create ajiro-api.yaml and the api/ directory (detects the framework)
    Init(InitArgs),
    /// Statically analyse routes and write/update endpoint YAML files
    Scan(ScanArgs),
    /// Run the requests in the YAML files and check the responses
    Test(TestArgs),
    /// List endpoints defined in the api/ directory
    List(ListArgs),
    /// Validate endpoint YAML files (syntax, schema and references)
    Validate(ValidateArgs),
    /// Serve the sample responses (`mock:`) of the YAML files
    Mock(MockArgs),
    /// Run the MCP server over stdio (tools: scan, test, list, validate)
    Mcp(McpArgs),
    /// Convert from/to OpenAPI 3.x
    #[command(subcommand)]
    Openapi(OpenapiCommand),
    /// Import Postman Collection v2.1 or Bruno collections
    #[command(subcommand)]
    Import(ImportCommand),
    /// Generate a single-file HTML API reference
    Docs(DocsArgs),
    /// Print the JSON Schema of endpoint files or of ajiro-api.yaml
    Schema(SchemaArgs),
    /// Show or change anonymous usage statistics consent (off by default)
    Telemetry(TelemetryArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Framework (auto-detected when omitted): nextjs, hono, fastapi, express, go-nethttp, go-chi, go-echo
    #[arg(long)]
    pub framework: Option<String>,
    /// Overwrite an existing ajiro-api.yaml
    #[arg(long)]
    pub force: bool,
    /// Base URL of the `local` environment (default depends on the framework)
    #[arg(long)]
    pub base_url: Option<String>,
    /// Machine-readable output
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Directories or files to scan (default: `sources` in ajiro-api.yaml)
    pub paths: Vec<PathBuf>,
    /// Framework (default: ajiro-api.yaml or auto-detect)
    #[arg(long)]
    pub framework: Option<String>,
    /// Show what would change without writing files
    #[arg(long)]
    pub dry_run: bool,
    /// Exit with code 1 if YAML files are out of date (for CI); writes nothing
    #[arg(long)]
    pub check: bool,
    /// With --check: do not count files whose only change is `source.line` as drift
    #[arg(long, requires = "check")]
    pub ignore_lines: bool,
    /// Machine-readable output
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct TestArgs {
    /// Only run endpoints/tests whose id, file or test name contains this text
    pub filter: Option<String>,
    /// Environment from ajiro-api.yaml (default: default_env)
    #[arg(long, short = 'e')]
    pub env: Option<String>,
    /// Override the environment base URL
    #[arg(long)]
    pub base_url: Option<String>,
    /// Machine-readable output: failures with expected/actual values and source locations
    #[arg(long)]
    pub json: bool,
    /// Also write a JUnit XML report to this file
    #[arg(long, value_name = "FILE")]
    pub junit: Option<PathBuf>,
    /// Overwrite stored snapshots with the actual responses
    #[arg(long, short = 'u')]
    pub update_snapshots: bool,
    /// Stop at the first failing test
    #[arg(long)]
    pub bail: bool,
    /// Request timeout in milliseconds
    #[arg(long)]
    pub timeout: Option<u64>,
    /// Print request/response details for passing tests too
    #[arg(long, short = 'v')]
    pub verbose: bool,
    /// Also send the implicit smoke request to endpoints without tests whose
    /// method may change data (POST, PUT, PATCH, DELETE). Off by default
    #[arg(long)]
    pub smoke_unsafe: bool,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Machine-readable output
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Machine-readable output
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct MockArgs {
    /// Port to listen on
    #[arg(long, short = 'p', default_value_t = 4010)]
    pub port: u16,
    /// Host to bind
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// Scenario name to use when an endpoint defines it (falls back to `default`)
    #[arg(long)]
    pub scenario: Option<String>,
    /// Extra latency for every response, in milliseconds
    #[arg(long)]
    pub delay: Option<u64>,
}

#[derive(Debug, Args)]
pub struct McpArgs {}

#[derive(Debug, Subcommand)]
pub enum OpenapiCommand {
    /// Create/update endpoint YAML files from an OpenAPI 3.x document (JSON or YAML)
    Import {
        file: PathBuf,
        /// Overwrite request/mock of existing files (tests are always kept)
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        json: bool,
    },
    /// Write an OpenAPI 3.1 document built from the endpoint YAML files
    Export {
        /// Output file (.json or .yaml); stdout when omitted
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        /// API title
        #[arg(long)]
        title: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ImportCommand {
    /// Import a Postman Collection v2.1 JSON file
    Postman {
        file: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        json: bool,
    },
    /// Import a Bruno collection directory (.bru files)
    Bruno {
        dir: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub struct DocsArgs {
    /// Output HTML file
    #[arg(long, short = 'o', default_value = "api-docs.html")]
    pub out: PathBuf,
    /// Document title
    #[arg(long)]
    pub title: Option<String>,
    /// Environment whose base URL is shown in examples
    #[arg(long, short = 'e')]
    pub env: Option<String>,
}

#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// Which schema: `endpoint` (api/**/*.yaml) or `config` (ajiro-api.yaml)
    #[arg(default_value = "endpoint")]
    pub which: String,
}

#[derive(Debug, Args)]
pub struct TelemetryArgs {
    /// `on`, `off` or `status`
    #[arg(default_value = "status")]
    pub action: String,
}

/// Resolve the working directory from `-C`.
pub fn work_dir(cli: &Cli) -> PathBuf {
    cli.dir.clone().unwrap_or_else(|| PathBuf::from("."))
}

pub fn main() -> i32 {
    let cli = Cli::parse();
    let dir = work_dir(&cli);
    let name = command_name(&cli.command);
    let update = crate::update_check::start(name, wants_json(&cli.command));
    let result = match &cli.command {
        Command::Init(a) => crate::init::run_cli(&dir, a),
        Command::Scan(a) => crate::scan::run_cli(&dir, a),
        Command::Test(a) => crate::runner::run_cli(&dir, a),
        Command::List(a) => crate::project_cli::list(&dir, a),
        Command::Validate(a) => crate::project_cli::validate(&dir, a),
        Command::Mock(a) => crate::mock::run_cli(&dir, a),
        Command::Mcp(_) => crate::mcp::run_cli(&dir),
        Command::Openapi(c) => crate::openapi::run_cli(&dir, c),
        Command::Import(c) => crate::importers::run_cli(&dir, c),
        Command::Docs(a) => crate::htmldoc::run_cli(&dir, a),
        Command::Schema(a) => crate::schema::run_cli(a),
        Command::Telemetry(a) => crate::telemetry::run_cli(a),
    };
    crate::telemetry::record(name);
    if let Some(u) = update {
        u.finish();
    }
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {:#}", e);
            if wants_json(&cli.command) {
                let obj = serde_json::json!({
                    "report_version": crate::report::REPORT_VERSION,
                    "ok": false,
                    "error": format!("{:#}", e),
                });
                println!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
            }
            crate::exit::ERROR
        }
    }
}

/// Whether the command was run with `--json` (errors then also go to stdout
/// as a JSON object).
fn wants_json(c: &Command) -> bool {
    match c {
        Command::Init(a) => a.json,
        Command::Scan(a) => a.json,
        Command::Test(a) => a.json,
        Command::List(a) => a.json,
        Command::Validate(a) => a.json,
        Command::Openapi(OpenapiCommand::Import { json, .. }) => *json,
        Command::Import(ImportCommand::Postman { json, .. })
        | Command::Import(ImportCommand::Bruno { json, .. }) => *json,
        _ => false,
    }
}

fn command_name(c: &Command) -> &'static str {
    match c {
        Command::Init(_) => "init",
        Command::Scan(_) => "scan",
        Command::Test(_) => "test",
        Command::List(_) => "list",
        Command::Validate(_) => "validate",
        Command::Mock(_) => "mock",
        Command::Mcp(_) => "mcp",
        Command::Openapi(_) => "openapi",
        Command::Import(_) => "import",
        Command::Docs(_) => "docs",
        Command::Schema(_) => "schema",
        Command::Telemetry(_) => "telemetry",
    }
}
