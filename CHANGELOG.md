# Changelog

All notable changes are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
