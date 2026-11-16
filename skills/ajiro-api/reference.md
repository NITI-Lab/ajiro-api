# ajiro-api YAML reference

Run `ajiro-api schema endpoint` or `ajiro-api schema config` for the exact JSON Schema. Unknown fields are errors (`deny_unknown_fields`), so a typo such as `capure:` fails `validate`.

## Endpoint file (`api/<resource>/<method>-<path>.yaml`)

```yaml
version: 1                 # required, always 1
id: users.create           # required, unique in the project
summary: Create a user     # optional
description: ...           # optional
tags: [users]              # optional
task: TSK-012              # optional (Ajiro task id)
depends_on: [auth.login.create]   # ids that run first; their captures are usable as {{ name }}
request:                   # required
  method: POST             # GET POST PUT PATCH DELETE HEAD OPTIONS (upper case)
  path: /users             # starts with /, params as {id}
  params: { id: "{{ userId }}" }   # values for {param} in path
  query: { page: 1 }
  headers:
    Authorization: Bearer {{ token }}
  body: { name: Taro, email: "{{ $randomEmail }}" }
source:                    # written by `scan`; NEVER edit
  framework: hono          # nextjs hono fastapi express go-nethttp go-chi go-echo
  file: src/routes/users.ts
  line: 42
  handler: createUser      # optional
tests: []                  # yours; empty = implicit smoke test (status < 500)
mock: {}                   # yours; used by `ajiro-api mock`
```

## Test case

```yaml
tests:
  - name: creates a user            # required, unique in file
    request:                        # optional overrides of `request`
      headers: { X-Trace: abc }     # params/query/headers merge key by key
                                    # header value "" removes the header (e.g. Authorization: "" to test 401)
      body: { name: Hanako }        # objects deep-merge; null removes a key; other types replace
    expect:
      status: 201                   # or [200, 204]
      headers:
        content-type: /^application\/json/   # /.../ = regex
      body:
        schema: ./schemas/user.json # path relative to this YAML, or an inline schema object
        match:                      # JSONPath -> value or matcher
          $.name: Hanako
          $.id: { $exists: true }
        equals: { ... }             # whole body (use sparingly)
        snapshot: true              # api/__snapshots__/<id>/<test-slug>.json
        contains: "ok"              # substring of a text body
      ignore: [$.id, $.createdAt]   # excluded from equals/snapshot
      max_ms: 500
    capture:
      userId: $.id                  # $.jsonpath | header:Name | status | body
      location: header:Location
    skip: false
    only_env: [local, ci]           # empty = all environments
```

## Matchers (exactly one key per object)

| Matcher | Meaning |
| --- | --- |
| `{ $exists: true }` | path present (`false`: absent) |
| `{ $type: string }` | `string` `number` `integer` `boolean` `array` `object` `null` |
| `{ $regex: "^usr_" }` | string matches regex |
| `{ $contains: x }` | substring / array element |
| `{ $len: 3 }` / `{ $len: { $gte: 1 } }` | length of array / string / object (exact, or a numeric matcher) |
| `{ $gt: 0 }` `{ $gte: 0 }` `{ $lt: 10 }` `{ $lte: 10 }` | numeric comparison |
| `{ $ne: x }` | not equal |
| `{ $oneOf: [open, done] }` | equals one of the values |

## JSONPath rules

- Syntax is RFC 9535: filters are `[?@.done == true]`, `[?@.price < 10 && @.tag == 'a']`, regex via `[?match(@.id, '^usr_')]` / `search(...)`. (`=~` is not supported.)
- A **singular** path (`$.a.b`, `$.items[0].id`) compares the single value.
- A path that can select **several** nodes (`[*]`, `..`, `[?...]`, slices) is compared as the **array of all matches**:
  - `$.items[*].id: { $contains: "{{ todoId }}" }` — some item has that id
  - `$.items[?@.done == true]: { $len: { $gte: 1 } }` — at least one done item
  - `$.items[?@.done == false]: { $len: 0 }` — none open
  - `{ $exists: false }` passes when nothing matches.

## Templates

- `{{ name }}`: captured variable or environment `variables` from `ajiro-api.yaml`.
- `{{ env.NAME }}`: value from the `.env` file, falling back to the process environment (so CI can pass secrets as env vars). Use it for secrets.
- Templates also work inside `match` keys: `$.items[?@.id == '{{ todoId }}'].title: Buy milk`.
- Built-ins: `{{ $uuid }}`, `{{ $timestamp }}`, `{{ $timestampMs }}`, `{{ $isoDate }}`, `{{ $randomInt }}`, `{{ $randomEmail }}`.

## Mock

```yaml
mock:
  default: { status: 201, body: { id: usr_001, name: Taro } }
  scenarios:
    conflict: { status: 409, body: { error: duplicate } }
    slow: { status: 200, delay_ms: 3000, headers: { X-Mock: "1" } }
```

Select a scenario with `ajiro-api mock --scenario conflict`, the `X-Ajiro-Scenario: conflict` header, or `?__scenario=conflict`.

## Project config (`ajiro-api.yaml`)

```yaml
version: 1
framework: hono            # optional; auto-detected
sources: [src]             # directories to scan
exclude: [node_modules]
api_dir: api
env_file: .env
default_env: local
environments:
  local:
    base_url: http://localhost:3000
    variables: { adminPassword: "{{ env.ADMIN_PASSWORD }}" }
    headers: {}            # sent with every request
  ci:
    base_url: http://localhost:3000
    env_file: .env.ci
redact: [x-session-id]     # extra names masked as ***
timeout_ms: 10000
```

## Example: auth chain

`api/auth/post-auth-login.yaml`

```yaml
version: 1
id: auth.login.create
request:
  method: POST
  path: /auth/login
  body: { username: demo, password: "{{ env.DEMO_PASSWORD }}" }
tests:
  - name: logs in
    expect:
      status: 200
      body:
        match: { $.token: { $type: string } }
    capture:
      token: $.token
```

`api/todos/post-todos.yaml`

```yaml
version: 1
id: todos.create
depends_on: [auth.login.create]
request:
  method: POST
  path: /todos
  headers: { Authorization: "Bearer {{ token }}" }
  body: { title: "Write tests {{ $uuid }}" }
tests:
  - name: creates a todo
    expect:
      status: 201
      body:
        match:
          $.title: { $regex: "^Write tests" }
          $.done: false
      ignore: [$.id, $.createdAt]
    capture:
      todoId: $.id
  - name: rejects an empty title
    request: { body: { title: "" } }
    expect: { status: 400 }
  - name: rejects a missing token
    request: { headers: { Authorization: "" } }
    expect: { status: 401 }
```

## Reading `test --json`

```json
{
  "report_version": 1, "env": "local", "base_url": "http://localhost:3000", "ok": false,
  "summary": { "total": 12, "passed": 11, "failed": 1, "skipped": 0, "errors": 0, "duration_ms": 84 },
  "results": [{
    "endpoint_id": "todos.create", "file": "api/todos/post-todos.yaml",
    "test": "creates a todo", "test_index": 0, "status": "failed",
    "request": { "method": "POST", "url": "...", "headers": { "authorization": "***" } },
    "response": { "status": 500, "headers": {}, "body": {} },
    "failures": [{ "check": "status", "expected": 201, "actual": 500, "message": "..." }],
    "source": { "framework": "hono", "file": "src/routes/todos.ts", "line": 31 }
  }],
  "load_errors": []
}
```

`status` is one of `passed`, `failed`, `skipped`, `error` (error = could not run: connection refused, template error). `failures[].check` is one of `status`, `header`, `body.equals`, `body.schema`, `body.match`, `body.snapshot`, `body.contains`, `max_ms`, `capture`.

## How `depends_on` executes

- Each endpoint file runs **once** per `test` run, all its tests in order. Order = topological by `depends_on`, otherwise by file path.
- Captures are **global** for the rest of the run: every dependent sees the same `{{ todoId }}`.
- So a destructive endpoint (DELETE) that uses a shared capture must depend on every endpoint that reads it, or it will delete the resource before they run:
  `depends_on: [todos.create, todos.get, todos.update, todos.list]`.
  Alternatively, let the DELETE file create its own resource in a first test (request override `method: POST`, `path: /todos`) and capture a separate variable.
- `ajiro-api test <filter>` also runs the dependencies of matching endpoints (with all their tests), so captures exist.
- Server state is shared across the run. Assert counts with `$gte` or capture a value before and compare after, or expose a test-only reset endpoint called by a first test.
