---
name: ajiro-api
description: Verify HTTP API endpoints with the ajiro-api CLI. Use whenever you implement, change or remove an HTTP API endpoint or route (Next.js Route Handlers, Hono, FastAPI, Express, Go net/http, chi, Echo), or when the user asks to test, verify, regression-check, mock or document an API. Scans routes into per-endpoint YAML, writes tests, runs them and self-fixes failures.
---

# ajiro-api: API verification loop

`ajiro-api` statically scans backend routes into one YAML file per endpoint (`api/<resource>/<method>-<path>.yaml`), runs those files as regression tests against a running server, and can serve them as mocks. Every command has a `--json` mode meant for you to read.

## When to run this

- Right after you add, change or delete an HTTP endpoint (route, handler, request/response shape, auth, status codes).
- When the user asks to "test the API", "check the endpoints", "make sure nothing broke", or wants a mock or API docs.
- Before you tell the user the API work is done.

Skip it for changes that cannot affect HTTP behaviour (docs, unrelated UI code).

## 0. Install check

```bash
ajiro-api --version || npx -y ajiro-api --version
```

If `ajiro-api` is not on PATH, use `npx -y ajiro-api <command>` everywhere below. If the project has no `ajiro-api.yaml`, run `ajiro-api init` once (it detects the framework and creates `ajiro-api.yaml` and `api/`).

## 1. The loop

After implementing or changing endpoints:

1. **Scan**: `ajiro-api scan --json`
   - Creates/updates YAML for new or changed routes. It only rewrites the `source:` block; it never touches `tests:`, `mock:` or comments.
   - `stale` in the report lists YAML files whose route no longer exists in code. They are never deleted automatically: if the route was removed on purpose, delete the file (and its `api/__snapshots__/<id>/`) yourself.
2. **Write tests** for every `created`/`updated` endpoint: add or extend `tests:` in its YAML (see [reference.md](reference.md)). Cover at least the success path, validation errors (400/422), auth failure (401/403) and not-found (404) where the code has them.
3. **Start the server** if it is not already running (in the background, e.g. the project's `dev` / `start` script) and wait until the port answers. The base URL comes from the environment in `ajiro-api.yaml` (`local` by default).
4. **Test**: `ajiro-api test --json` (add a filter to focus, e.g. `ajiro-api test todos --json`).
   - Exit code `0` all passed, `1` some tests failed, `2` the tool itself could not run (bad config, server down, unreadable YAML).
5. **Read failures**: for each result with `status` `failed` or `error`, read `failures[]` (`check`, `path`, `expected`, `actual`, `message`), the logged `request`/`response`, and `source.file` + `source.line` to open the handler.
6. **Decide which side is wrong**, then fix it:
   - The code contradicts the intended behaviour -> fix the code.
   - The test expectation is wrong or outdated (the behaviour change is intended) -> fix the test.
   - Unsure which is intended -> ask the user; do not guess.
7. **Re-run** `ajiro-api test --json` until everything is green. After code changes, run `ajiro-api scan --json` again so `source:` line numbers stay current.
8. **Validate**: `ajiro-api validate` (syntax, schema, duplicate ids, unknown `depends_on`).
9. **Commit the YAML together with the code** (`api/**` including `api/__snapshots__/**`). In CI, `ajiro-api scan --check` fails when the YAML is out of date.

## 2. Rules

- **Never edit `source:`.** `scan` owns it. Everything else (`tests`, `mock`, `summary`, `tags`, `depends_on`) is yours.
- **Never put secrets in YAML.** Use `{{ env.NAME }}` (read from `.env`, falling back to process env vars) or environment `variables` in `ajiro-api.yaml`. Do not commit `.env`. Secrets are masked as `***` in output.
- **Use `ignore` for volatile fields** (`$.id`, `$.createdAt`, tokens) when using `equals` or `snapshot`.
- **Prefer `match` + `schema` over `equals`.** Assert the fields that matter instead of the whole body.
- **Use `depends_on` + `capture` for auth chains.** Capture the token once in `auth.login...` and use `{{ token }}` in dependents. Read "How `depends_on` executes" in reference.md before writing DELETE tests: captures are shared, so order matters.
- **`scan` writes `tests: []`.** Replace that line with your list; do not add a second `tests:` key (it fails validation).
- **Do not weaken assertions just to get green.** Do not delete failing tests, loosen status checks, or add `skip: true` to hide a real bug. If a test is wrong, fix it to express the intended behaviour.
- **Update snapshots only when the change is intended**: `ajiro-api test -u` overwrites stored snapshots. Review the diff of `api/__snapshots__/` before committing.
- Tests that create data should use unique values (`{{ $randomEmail }}`, `{{ $uuid }}`) so re-runs do not collide.
- An endpoint with empty `tests` only gets an implicit `smoke` test (status < 500; a 4xx passes with a `warnings` entry — a 401/403 there means the run is not logged in, so fix `setup`/`headers` first), and only for GET/HEAD/OPTIONS: POST/PUT/PATCH/DELETE are skipped unless `--smoke-unsafe`. That is not enough for new work.
- **When most endpoints need a login**, put the login endpoint in the environment `setup` and send the captured token or cookie (`cookie:<name>` capture) through the environment `headers`, instead of adding `depends_on` to every file.

## 3. Other commands

| Command | Use |
| --- | --- |
| `ajiro-api list --json` | Endpoints, ids, test counts, source locations |
| `ajiro-api schema endpoint` / `schema config` | JSON Schema of the YAML files (check syntax) |
| `ajiro-api scan --check` | CI: exit 1 if YAML is out of date (`--ignore-lines`: not when routes only moved) |
| `ajiro-api test --env ci --junit report.xml` | CI run with JUnit output |
| `ajiro-api mock` | Serve `mock:` responses on port 4010 (scenarios: `--scenario`, `X-Ajiro-Scenario` header, `?__scenario=`) |
| `ajiro-api openapi export -o openapi.yaml` / `openapi import <file>` | OpenAPI 3.x conversion |
| `ajiro-api import postman <file>` / `import bruno <dir>` | Migrate existing collections |
| `ajiro-api docs -o api-docs.html` | Single-file HTML API reference |

If the ajiro-api MCP server is registered (the plugin from the `NITI-Lab/ajiro-api` marketplace registers `ajiro-api mcp`), the tools `scan`, `test`, `list`, `validate`, `get_endpoint` and `schema` do the same as the commands above.

Full YAML syntax and examples: [reference.md](reference.md).
