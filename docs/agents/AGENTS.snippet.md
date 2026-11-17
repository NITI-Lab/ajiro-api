## API verification with ajiro-api

Use `ajiro-api` whenever you add, change or remove an HTTP API endpoint (Next.js Route Handlers, Hono, FastAPI, Express, Go net/http / chi / Echo), or when asked to test, verify or document an API. It scans routes into one YAML file per endpoint (`api/<resource>/<method>-<path>.yaml`), runs them as regression tests against a running server, and can serve them as mocks. Every command has a `--json` mode: read it instead of scraping text.

### Install check

```bash
ajiro-api --version || npx -y ajiro-api --version
```

If `ajiro-api` is not on PATH, use `npx -y ajiro-api <command>` instead. If `ajiro-api.yaml` does not exist, run `ajiro-api init` once.

### The loop (run after every endpoint change)

1. `ajiro-api scan --json`: creates/updates YAML for new or changed routes. It only rewrites the `source:` block and never touches `tests:`, `mock:` or comments. `stale` lists YAML files whose route is gone; they are never deleted automatically, so remove them yourself if the route was removed on purpose.
2. Write or extend `tests:` in the YAML of every created/updated endpoint. Cover the success path, validation errors (400/422), auth failures (401/403) and not-found (404) where the code has them.
3. Start the server if it is not running (in the background, e.g. the `dev`/`start` script) and wait until the port answers. The base URL is the `local` environment in `ajiro-api.yaml`.
4. `ajiro-api test --json` (filter with `ajiro-api test <text> --json`). Exit code: `0` all passed, `1` tests failed, `2` the tool could not run (bad config, server down, unreadable YAML).
5. For each result with status `failed` or `error`, read `failures[]` (`check`, `path`, `expected`, `actual`, `message`), the logged `request`/`response`, and `source.file` + `source.line` (the handler location).
6. Decide whether the code or the test is wrong. Fix the code if it contradicts intended behaviour; fix the test only if the behaviour change is intended. Ask the user if unsure.
7. Re-run `ajiro-api test --json` until green. Run `ajiro-api scan --json` again after code changes so `source:` stays current.
8. `ajiro-api validate` (syntax, schema, duplicate ids, unknown `depends_on`).
9. Commit the YAML (`api/**`, including `api/__snapshots__/**`) together with the code. CI runs `ajiro-api scan --check`, which fails when YAML is out of date.

### Rules

- Never edit `source:`. `scan` owns it.
- Never put secrets in YAML. Use `{{ env.NAME }}` (read from `.env` only) or environment `variables` in `ajiro-api.yaml`. Do not commit `.env`.
- Use `expect.ignore` for volatile fields (`$.id`, `$.createdAt`, tokens) when using `equals` or `snapshot`.
- Prefer `body.match` + `body.schema` over `body.equals`.
- Use `depends_on` + `capture` for auth chains (capture the token in the login endpoint, use `{{ token }}` in dependents).
- Do not weaken assertions just to get green: do not delete failing tests, loosen status checks, or use `skip: true` to hide a real bug.
- Update snapshots (`ajiro-api test -u`) only when the change is intended, and review the diff.
- Use unique values for created data (`{{ $randomEmail }}`, `{{ $uuid }}`) so re-runs do not collide.
- An endpoint with empty `tests` only gets an implicit smoke test (status < 500), and only for GET/HEAD/OPTIONS: POST/PUT/PATCH/DELETE are skipped unless `--smoke-unsafe`. That is not enough for new work.
- When most endpoints need a login, list the login endpoint in the environment `setup` and send the captured token or cookie (`cookie:<name>`) through the environment `headers`.

### YAML cheat sheet

```yaml
version: 1
id: todos.create               # unique; used by depends_on and snapshots
depends_on: [auth.login.create]
request:
  method: POST                 # upper case
  path: /todos                 # {param} for path params
  headers: { Authorization: "Bearer {{ token }}" }
  body: { title: "Write tests {{ $uuid }}" }
source:                        # written by scan; do not edit
  framework: hono
  file: src/routes/todos.ts
  line: 31
tests:
  - name: creates a todo
    expect:
      status: 201                        # or [200, 204]
      headers: { content-type: /^application\/json/ }   # /.../ = regex
      body:
        schema: ./schemas/todo.json      # path relative to the YAML, or inline schema
        match:                           # JSONPath -> value or matcher
          $.title: { $regex: "^Write tests" }
          $.id: { $exists: true }
        snapshot: false
      ignore: [$.id, $.createdAt]
      max_ms: 500
    capture: { todoId: $.id }            # $.jsonpath | header:Name | cookie:name | status | body
  - name: rejects an empty title
    request: { body: { title: "" } }     # merged over `request`
    expect: { status: 400 }
mock:
  default: { status: 201, body: { id: todo_001 } }
  scenarios:
    conflict: { status: 409 }
```

Matchers (one key per object): `$exists`, `$type` (`string|number|integer|boolean|array|object|null`), `$regex`, `$contains`, `$len`, `$gt`, `$gte`, `$lt`, `$lte`, `$ne`, `$oneOf`.

Templates: `{{ var }}` (captured or environment variable), `{{ env.NAME }}` (`.env` only), `{{ $uuid }}`, `{{ $timestamp }}`, `{{ $timestampMs }}`, `{{ $isoDate }}`, `{{ $randomInt }}`, `{{ $randomEmail }}`.

Syntax errors: unknown fields are rejected. Check with `ajiro-api validate`, or read the JSON Schema with `ajiro-api schema endpoint` / `ajiro-api schema config`.

More: `ajiro-api list --json`, `ajiro-api mock` (port 4010; scenarios via `--scenario`, `X-Ajiro-Scenario` header or `?__scenario=`), `ajiro-api openapi export|import`, `ajiro-api import postman|bruno`, `ajiro-api docs`. An MCP server is available with `ajiro-api mcp` (tools: scan, test, list, validate, get_endpoint, schema).
