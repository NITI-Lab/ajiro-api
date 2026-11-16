# Changelog

All notable changes are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - unreleased

Initial release (Phase 1).

### Added

- `init`, `scan`, `test`, `list`, `validate` commands, each with `--json` output (report contract version 1).
- Route scanning for Next.js Route Handlers, Hono and FastAPI; Express and Go (net/http, chi, Echo) as follow-ups.
- Endpoint YAML format (`version: 1`) and project config `ajiro-api.yaml`, published as JSON Schemas (`schema/`).
- Assertions: status, headers, `equals`, JSON Schema, JSONPath `match` with matchers, snapshots (`-u` to update), `ignore`, `max_ms`.
- Templates (`{{ var }}`, `{{ env.NAME }}`, built-ins), `capture` and `depends_on` for request chains, environments and `.env` secrets with masking.
- `scan --check` and `test --junit` for CI; GitHub Actions sample.
- `mock` server with scenarios; `mcp` server; `openapi import/export`; `import postman/bruno`; `docs` (single-file HTML).
- Claude Code plugin and skill, Codex `AGENTS.md` snippet.
- Distribution via GitHub Releases, npm (`npx ajiro-api`), Homebrew and cargo.
- Anonymous usage statistics, off by default (`ajiro-api telemetry`).
