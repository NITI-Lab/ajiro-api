# ajiro-api

**Let your coding agent test the APIs it writes.**

Ajiro API is a CLI that statically scans your backend routes into one YAML file per endpoint, runs those files as regression tests against your dev server, and serves them as mocks. It is built so that Claude Code and Codex can run "implement -> scan -> test -> fix -> re-test" with no human in the loop.

![Demo: scan and test pass, a moved route makes GET /todos/stats return 404, the fix makes all 34 tests pass again](https://raw.githubusercontent.com/NITI-Lab/ajiro-api/main/docs/demo/demo.gif)

- **Built for agents**: `test --json` returns the failing check, expected and actual values, the request, the response body and the handler's `file:line`.
- **Your tests are safe**: `scan` only rewrites the `source:` block; tests, mocks, comments and key order are never touched.
- **Plain YAML, one file per endpoint**: fewer Git conflicts, a narrow surface for AI to edit, convertible to and from OpenAPI.
- **Local-first**: API definitions and test results never leave your machine. Telemetry is off by default.
- **Single native binary**: no Node or Python needed at runtime. MIT licensed.

Supports **Next.js Route Handlers, Hono, FastAPI, Express and Go (net/http, chi, Echo)**.

## Quick start

```bash
cd your-backend-project
npx -y ajiro-api init     # detect the framework, create ajiro-api.yaml and api/
npx -y ajiro-api scan     # one YAML file per endpoint under api/
# start your app, then
npx -y ajiro-api test     # run every endpoint and check the responses
```

`npx -y ajiro-api` fetches the latest release on every run. In CI and other automated runs, pin the version: `npx -y ajiro-api@0.2.4 test`, or `npm i -D ajiro-api@0.2.4` and let the lockfile pin it.

Endpoints without tests get a safe smoke check (GET, HEAD and OPTIONS only). Add `tests:` yourself, or ask your agent to.

## What a test looks like

```yaml
# api/users/post-users.yaml
version: 1
id: users.create
request:
  method: POST
  path: /users
  headers:
    Authorization: Bearer {{ token }}
  body: { name: Taro, email: taro@example.com }
source:            # written by `ajiro-api scan`
  framework: hono
  file: src/routes/users.ts
  line: 42
tests:             # written by you or your agent
  - name: creates a user
    expect:
      status: 201
      body:
        match:
          $.email: taro@example.com
    capture:
      userId: $.id
  - name: duplicate email returns 409
    request:
      body: { email: dup@example.com }
    expect:
      status: 409
mock:              # served by `ajiro-api mock`
  default: { status: 201, body: { id: usr_001, name: Taro } }
```

## Use it from Claude Code, Codex or any MCP client

Claude Code plugin (a skill for the scan -> test -> fix loop, plus the MCP server):

```text
/plugin marketplace add niti-lab/ajiro-api
/plugin install ajiro-api@ajiro-api
```

MCP server only: it is listed in the [official MCP Registry](https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.NITI-Lab/ajiro-api) as `io.github.NITI-Lab/ajiro-api` and on [Glama](https://glama.ai/mcp/servers/NITI-Lab/ajiro-api). Add it to `.mcp.json` or your client's MCP settings:

```json
{
  "mcpServers": {
    "ajiro-api": { "command": "npx", "args": ["-y", "ajiro-api", "mcp"] }
  }
}
```

Codex: paste [AGENTS.snippet.md](https://github.com/niti-lab/ajiro-api/blob/main/docs/agents/AGENTS.snippet.md) into your `AGENTS.md`.

## Commands

| Command | What it does |
| --- | --- |
| `init` | Create `ajiro-api.yaml` and `api/`, detect the framework |
| `scan` | Write or update one YAML file per route. `--check` fails CI when files are out of date |
| `test` | Run the YAML against a server. `--json`, `--junit <file>`, `--env`, `-u` (update snapshots) |
| `mock` | Serve the `mock:` responses, with scenarios and delays |
| `docs` | Generate a single-file HTML API reference to send to clients |
| `openapi import/export` | Convert from/to OpenAPI 3.x |
| `import postman/bruno` | Migrate Postman collections or Bruno folders |
| `mcp` | Run the MCP server over stdio |

Exit codes: `0` success, `1` tests failed / drift / validation errors, `2` the tool could not run.

## Install options

- `npx -y ajiro-api` (this package)
- `brew install niti-lab/tap/ajiro-api`
- `cargo install ajiro-api`
- Binaries for macOS, Linux and Windows on [GitHub Releases](https://github.com/niti-lab/ajiro-api/releases)

This npm package is a small launcher. The native binary comes from a platform package (`@niti-lab/ajiro-api-<platform>-<arch>`) installed automatically as an optional dependency, for Linux x64/arm64, macOS x64/arm64 and Windows x64.

## Links

- Documentation and source: https://github.com/niti-lab/ajiro-api
- YAML reference: [docs/yaml-reference.md](https://github.com/niti-lab/ajiro-api/blob/main/docs/yaml-reference.md) (Japanese), [skills/ajiro-api/reference.md](https://github.com/niti-lab/ajiro-api/blob/main/skills/ajiro-api/reference.md) (English)
- Issues: https://github.com/niti-lab/ajiro-api/issues
- Made by [NITI](https://nititech.jp/)

MIT License.
