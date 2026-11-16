# Contributing

Thanks for helping with Ajiro API. Issues and pull requests are welcome in Japanese or English.

## Setup

You need a recent stable Rust toolchain (the minimum supported version is in `Cargo.toml`).

```bash
git clone https://github.com/niti-lab/ajiro-api
cd ajiro-api
cargo build
cargo test
```

## Branches

- `main`: release branch. Tags `v*` on `main` trigger releases.
- `develop`: integration branch. Open pull requests against `develop`.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks on Linux, macOS and Windows.

## Where things live

| Area | Location |
| --- | --- |
| Data model (YAML), config, naming, report contract | `src/model.rs`, `src/config.rs`, `src/project.rs`, `src/naming.rs`, `src/report.rs` |
| Route discovery (`init`, `scan`) | `src/init.rs`, `src/scan/` (one module per framework) |
| `test` command | `src/runner/` |
| Integrations | `src/mcp.rs`, `src/mock.rs`, `src/openapi.rs`, `src/importers/`, `src/htmldoc.rs` |
| JSON Schemas | `schema/*.schema.json` |
| Claude Code plugin and skill | `.claude-plugin/`, `.mcp.json`, `skills/ajiro-api/` |
| Codex instructions | `docs/agents/AGENTS.snippet.md` |
| npm / Homebrew packaging | `npm/`, `packaging/homebrew/` |
| Examples | `examples/` |

## Rules that matter

- **`--json` output is a contract.** `src/report.rs` is read by agents, the MCP server and CI. Add fields; do not rename or remove them without bumping `REPORT_VERSION` and noting it in `CHANGELOG.md`.
- **`scan` must not rewrite human-written parts.** It only rewrites the top-level `source:` block. Never `tests`, `mock`, comments or key order. It reports stale files and never deletes them.
- **Keep the docs in sync.** A change to `src/model.rs` means updating `schema/*.schema.json`, `docs/yaml-reference.md`, `skills/ajiro-api/reference.md` and `docs/agents/AGENTS.snippet.md`. A change to `src/cli.rs` means updating `docs/cli-reference.md` and the READMEs.
- **Adding a framework** needs a fixture project, a scan snapshot test and an entry in the supported-frameworks tables.
- **No secrets in output.** Tokens and passwords are masked as `***`.
- **Telemetry stays opt-in** and limited to command counts.
- **Fast startup.** `--version` should stay well under 0.1 s.

## Reporting issues

Use the issue templates. For requests about a GUI, mock server features or spec documents, use the "GUI / mock / spec request" template; we count these to decide what to build next. Security problems: see [SECURITY.md](SECURITY.md).

## License

By contributing you agree that your contribution is licensed under the [MIT License](LICENSE).
