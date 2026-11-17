# Changelog

All notable changes are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- The npm platform packages that carry the native binary are named `@niti-lab/ajiro-api-<platform>-<arch>` (previously planned as `@ajiro-api/cli-<platform>-<arch>`, never published). `npx ajiro-api` is unchanged.

## [0.1.5] - 2026-11-17

### Changed

- **YAML is read and written with [serde-yaml-rust2](https://crates.io/crates/serde-yaml-rust2)** (pure Rust, no `unsafe`) instead of the archived `serde_yaml` / `unsafe-libyaml`. A mapping with the same key twice (for example two `$.total` entries under `match`) is now an error instead of the last entry silently replacing the earlier one. Files written by `openapi import`, `postman import` and `bruno import` indent sequences under their key (`tags:\n  - a`), like the files `init` and the examples use.

### Fixed

- `$regex` matchers and `/.../` header patterns are compiled with a 1 MB size limit, so an oversized pattern fails the check instead of using unbounded memory.
- The mock server keeps serving if a thread panics while holding its route registry lock, instead of every later request panicking too.
- `--version` reports the release version again (the v0.1.3 and v0.1.4 binaries printed `0.1.2`).

## [0.1.4] - 2026-11-17

The v0.1.3 tag points at the same commit as 0.1.2.

### Changed

- **The implicit `smoke` test is only sent to GET, HEAD and OPTIONS.** Endpoints without tests whose method may change data (POST, PUT, PATCH, DELETE) are now skipped with a reason, instead of receiving an empty request that could log out, overwrite or create data on the server under test. Pass `ajiro-api test --smoke-unsafe` (MCP: `smoke_unsafe: true`) for the previous behaviour.
- Environment `headers` are rendered per request, so they can use values captured during the run (e.g. `Cookie: "session={{ session }}"`). `{{ env.X }}` still has to resolve when the run starts.

### Added

- `environments.<name>.setup`: endpoint ids (e.g. a login) run before every other endpoint, with their `depends_on`, even when `test` is given a filter. A failure stops the run. While setup runs, an environment header whose captured value does not exist yet is left out. `validate` reports unknown ids.
- `capture: { name: cookie:<cookie-name> }` reads a cookie from the `Set-Cookie` response headers (the last one wins). Captured cookies are always masked in logs.

## [0.1.2] - 2026-11-17

### Fixed

- Hono scan: `.openapi(routes.list, handler)` now resolves `createRoute` objects grouped in a const object literal (local or imported through named, default, namespace imports and re-exports; `obj.prop`, `obj["prop"]`, nested `a.b.c`, shorthand properties, inline `createRoute({...})` values, `as const` / `satisfies` wrappers), so these routes keep their mount prefix instead of falling back to the orphan path.
- Hono / Express scan: a `.on()` / `.get()` / ... registration whose handlers are all call expressions (`adminChecker("X")`) and whose paths are all registered by another registration on the same router is treated as middleware and no longer produces routes (the methods x paths cross product). Call-only handlers on paths nothing else registers (`app.get("/reference", apiReference(...))`) are still routes.
- `normalize_path` never emits an empty `{}` parameter any more: a parameter without a recoverable name (`/:`, `/:{regex}`, `/{...}`, `/[]`) becomes `{param}` and scan warns with the raw path. Hono regex params containing `/` (`:id{[^/]+}`) are no longer split, and array paths with elements that are not statically known now warn instead of being dropped silently.

## [0.1.1] - 2026-11-17

### Fixed

- `openapi import` (and Postman/Bruno import) no longer drops an endpoint whose file name collides with another one (`/a/b-c` vs `/a/b/c`); the second file gets a `-2` suffix.
- Closing stdout early (`ajiro-api list | head`) no longer panics with "Broken pipe".

### Changed

- Hono scan supports `@hono/zod-openapi` projects: apps created by factory functions (`createApp().basePath("/api")`, transitively and across files), `createRoute({...})` defined in other files and registered with `.openapi(route, handler)` (named, namespace and re-exported imports), template-literal paths built from string consts, `routes.forEach(r => app.route("/", r))` mounts and `app.doc()`. `createRoute` definitions that cannot be tied to an app are still emitted, with a warning. A scan that finds no routes now suggests `openapi import`.
- `init` detects the dev server port (`.env.example` `PORT=`, `-p`/`--port` in scripts, `serve({ port })`, `process.env.PORT ?? 8000`, `.listen()`, `ListenAndServe(":8080")`, `uvicorn.run(port=)`) and prints where it found it.

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
