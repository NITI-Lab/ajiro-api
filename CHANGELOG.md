# Changelog

All notable changes are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-16

Initial release (Phase 1).

### Added

- `init`, `scan`, `test`, `list`, `validate` commands, each with `--json` output (report contract version 1).
- Route scanning for Next.js Route Handlers (App and Pages Router), Hono, FastAPI, Express and Go (net/http, chi, Echo).
- Endpoint YAML format (`version: 1`) and project config `ajiro-api.yaml`, published as JSON Schemas (`schema/`).
- Assertions: status, headers, `equals`, JSON Schema, JSONPath `match` with matchers, snapshots (`-u` to update), `ignore`, `max_ms`.
- Templates (`{{ var }}`, `{{ env.NAME }}`, built-ins), `capture` and `depends_on` for request chains, environments and `.env` secrets with masking.
- `scan --check` and `test --junit` for CI; GitHub Actions sample.
- `mock` server with scenarios; `mcp` server; `openapi import/export`; `import postman/bruno`; `docs` (single-file HTML).
- Claude Code plugin and skill, Codex `AGENTS.md` snippet.
- Distribution via GitHub Releases, npm (`npx ajiro-api`), Homebrew and cargo.
- Anonymous usage statistics, off by default (`ajiro-api telemetry`).
- JSONPath `match` on multi-node paths (`[*]`, `..`, filters) compares the array of all matches; templates allowed in `match` keys; `$len` accepts a numeric matcher (`{ $len: { $gte: 1 } }`); a header value of `""` in a test removes the header.

### Fixed

- JSON Schema `$ref` to sibling files failed on Windows (malformed `file://` base URI).
