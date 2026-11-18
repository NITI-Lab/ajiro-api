# AGENTS.md (contributors)

Guidance for AI agents and humans working on this repository (`ajiro-api`, a Rust CLI). `CLAUDE.md` points here.

## What this is

`ajiro-api` statically scans backend routes (Next.js Route Handlers, Hono, FastAPI, Express, Go net/http / chi / Echo) into one YAML file per endpoint, runs the YAML as regression tests against a running server, and serves mocks. It is built so that coding agents can run "implement -> scan -> test -> fix -> re-test" without a human. Requirements (Japanese): `docs/requirements.md`.

## Architecture (module ownership, see `src/lib.rs`)

- Core: `model` (YAML data model, `deny_unknown_fields`), `config`, `project` (loading and validation), `naming` (file names and endpoint ids), `report` (the `--json` contract), `cli`, `telemetry`, `schema`.
- Route discovery: `init`, `scan` (tree-sitter based, one module per framework under `src/scan/`).
- `runner`: the `test` command (HTTP execution, templates, assertions, snapshots, reporters).
- Integrations: `mcp`, `mock`, `openapi`, `importers` (Postman, Bruno), `htmldoc`.
- `schema/*.schema.json` are the published JSON Schemas; they are `include_str!`ed by `src/schema.rs` and must stay in sync with `src/model.rs`.
- Other: `skills/ajiro-api/` (Claude Code skill), `plugins/ajiro-api/` (skill-only copy submitted to the Claude plugin directory; `skills/` and the icon are copies of the root ones, CI checks they match), `.claude-plugin/`, `.mcp.json`, `docs/agents/AGENTS.snippet.md` (Codex), `npm/` (npm distribution), `packaging/homebrew/`, `examples/`.

## Commands

```bash
cargo build
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo run -- --help
```

CI runs fmt, clippy (warnings are errors) and tests on Linux, macOS and Windows.

## Conventions

- **The report JSON is a contract.** Types in `src/report.rs` are consumed by agents, the MCP server and CI. Add fields freely; never rename or remove one without bumping `REPORT_VERSION` and `CHANGELOG.md`.
- **`scan` must not rewrite human-written parts.** It only rewrites the top-level `source:` block textually. It never touches `tests`, `mock`, comments or key order, and never deletes files (it reports `stale`). Add a test for any change near this.
- Changing `src/model.rs` means updating `schema/*.schema.json`, `docs/yaml-reference.md`, `skills/ajiro-api/reference.md` and `docs/agents/AGENTS.snippet.md` in the same change.
- After editing `skills/` or `.claude-plugin/icon.png`, copy them into `plugins/ajiro-api/`. On release, bump the version in `plugins/ajiro-api/.claude-plugin/plugin.json` and `server.json` too.
- Changing CLI flags in `src/cli.rs` means updating `docs/cli-reference.md` and both READMEs.
- Exit codes: `0` ok, `1` tests failed / drift / validation errors, `2` the tool itself could not do its job.
- Secrets are masked as `***` in all output. Never log tokens or passwords.
- Telemetry is off by default and counts command names only. Do not add anything that sends data without explicit consent.
- Keep startup fast (target: `--version` under 0.1 s) and scan of 100 endpoints under 3 s.
- Each supported framework needs a fixture project and a scan snapshot test.
- Do not add runtime dependencies on Node or Python; the binary must be self-contained.
- Commit messages: short imperative subject. Branch from `develop`; `main` is the release branch.
