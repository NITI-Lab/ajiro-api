# Changelog

All notable changes are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- The READMEs mention the official MCP Registry and Glama listings (with badges) and `claude mcp add` for the MCP server alone.
- `plugins/ajiro-api/`: a skill-only plugin bundle for the Claude plugin directory. The marketplace plugin at the repository root (skill + MCP server) is unchanged.

## [0.2.3] - 2026-11-18

### Added

- `server.json` for the official MCP Registry; the server is listed as `io.github.NITI-Lab/ajiro-api`.
- A plugin icon (`.claude-plugin/icon.png`) for the Claude plugin directory.

## [0.2.2] - 2026-11-18

### Changed

- Package metadata: the npm README is a full introduction (demo, quick start, YAML example, Claude Code / MCP / Codex setup, commands), the npm and crates.io descriptions and keywords cover what the tool does, the platform packages get keywords and a README pointing to `ajiro-api`, and the homepage is https://nititech.jp/ (the repository link is unchanged).

## [0.2.1] - 2026-11-18

### Added

- The npm package declares `mcpName: io.github.NITI-Lab/ajiro-api`, so the MCP server can be listed in the official MCP Registry.
- A demo GIF at the top of the READMEs (`docs/demo/demo.gif`, recorded with `docs/demo/demo.tape`).

### Changed

- The README framework table no longer lists Express and Go as "within a month"; all seven frameworks are supported.
- `docs/requirements.md` is a public version that lists what ships today.

## [0.2.0] - 2026-11-18

### Added

- `scan --check --ignore-lines`: files whose only change is `source.line` (any edit above a route shifts it) do not count as drift, so CI does not fail on unrelated edits. New routes and changed files or handlers still do. The scan report gains `line_only` (count) and `endpoints[].line_only`.
- A smoke check that passes with a 4xx now carries `warnings` in the test report (`results[].warnings`, `summary.warnings`), shown as `warning:` lines and in JUnit `<system-out>`. A 401/403/407 says the request was not authenticated, not allowed, or rejected by a middleware (CSRF/origin guard, ...), and points at `environments.<env>.setup` / `headers` for the login case. Exit codes are unchanged.
- `init` warns when no supported framework is detected: the JSON report has a new `warnings` array, and recognisable unsupported stacks are named (NestJS, Fastify, Koa, Flask, Django, AWS Amplify Gen2, Cloudflare Pages Functions, Google Apps Script). Exit code stays 0.
- With `--json`, failures that exit 2 also print `{"report_version":1,"ok":false,"error":"..."}` on stdout (stderr keeps the text).
- Next.js: a static string-literal `basePath` from `next.config.{js,mjs,ts,cjs,mts,cts}` is prepended to the routes of that app (`/admin/api/...`); a non-literal `basePath` warns.
- Hono / Express: import aliases follow the nearest `tsconfig.json` / `jsconfig.json` `compilerOptions.paths` and `baseUrl` (relative `extends`, JSONC), keeping the `@/` -> `src/` fallback.
- FastAPI: include chains are followed through `for router in (...)` loops, `setup(app)` helper functions and `app = create_app()` factories; a mounted FastAPI sub-app is scanned with its mount prefix.
- Scan warnings for things that are not listed as routes or lose information: duplicate method + path (both `file:line`), more than one Next.js app in one scan (use one `ajiro-api.yaml` per app), catch-all parameters (`[...slug]`, `{name:path}`) that match several segments, FastAPI websocket routes and mounts.

### Changed

- `README.md` is now the English README; the Japanese one moved to `README.ja.md`.
- Paths made only of parameters after the `api`/`v1` prefix (catch-all routes such as `/api/v1/{resource}/{id}`) are named from the parameter names: `resource.get`, `resource.id.get`, ... in `api/resource/`, instead of `root.get`, `root.get-2`, ... in `api/root/`. Existing files keep their names.
- Endpoint ids for new files no longer depend on scan order. When base ids collide, the plain route keeps it and the others get the full form (the `api`/`vN` prefix and parameter names kept, e.g. `medical.offices.medical-office-id.users.list`, `api.health.get`); `-2` is used only when the full forms are identical too. File-name suffixes are assigned in a stable (path, method) order. Existing ids are unchanged.
- A GET whose last segment is static is `list` only when it looks like a collection (`users`, `logs`, `search`, `history`, ...); otherwise `get` (`health`, `me`, `detail`). Doubled verbs (`offers.create.create`, `units.list.list`) are no longer produced, and a segment mixing a parameter and text (`{item_id}.png`) gives `w.og.png.get`.
- `scan` skips files ignored by `.gitignore` (files tracked by git are always scanned) and test code by default (`tests/`, `test/`, `__tests__/`, `test_*.py`, `*_test.py`, `conftest.py`, `*_test.go`, `*.test.*`, `*.spec.*`; Next.js route folders named `test` are kept). Vendored trees ignored by git no longer slow scans down.
- FastAPI: only routes reachable from a module-level or factory-returned app via `include_router` / mount are emitted; routers that are never included and apps created inside functions (tests) are skipped with one warning. Projects without any app still emit every router, with a warning. Routes are returned in registration order, so the first duplicate is the one FastAPI serves.
- The "no routes found" warning counts only parsed source files, lists the supported frameworks and names a recognised unsupported stack. `scan --check` exits 1 when no routes are found and no endpoint files exist (nothing was checked).
- `init` does not pick `hono` for browser apps that only use `hono/client`, and prefers "NestJS (unsupported)" over `express` when `@nestjs/core` is present.
- Directory listings are cached during a scan, so marker-file lookups (`next.config.*`, `tsconfig.json`, `.gitignore`) cost one listing per directory; this matters on network and WSL mounts.

### Fixed

- An existing but invalid `ajiro-api.yaml` (for example missing `version`) is an error (exit 2) for every command instead of a silent fallback to defaults that then reported "could not detect the framework".
- Scans with several `sources` no longer silently drop routes that share a method and path; the first in `sources` (or registration) order wins and every dropped one is reported.
- FastAPI: trailing slashes are preserved (`/items/` and `/items` are distinct routes; `@router.get("/")` on a prefixed router keeps its slash instead of producing a path that only redirects). Re-scans match existing files with the slash, and the mock server prefers the route whose trailing slash matches the request.
- Hono: app factories that wrap the constructor (`wrap(new OpenAPIHono())`) are recognised; instances returned from factory functions keep the prefix they are mounted with; routes of factories that are never called are no longer emitted (a warning is shown). Clearer warnings for unresolved mounts, deduplicated.
- Hono / Express: tsconfig `paths` aliases resolve on Windows.
- `source.file` is a clean path for sources outside the project root (no more `//mnt/...`).

## [0.1.6] - 2026-11-17

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
