# Ajiro API

**Let your coding agent test the APIs it writes.**

[![npm](https://img.shields.io/npm/v/ajiro-api?label=npm)](https://www.npmjs.com/package/ajiro-api) [![crates.io](https://img.shields.io/crates/v/ajiro-api)](https://crates.io/crates/ajiro-api) [![CI](https://github.com/niti-lab/ajiro-api/actions/workflows/ci.yml/badge.svg)](https://github.com/niti-lab/ajiro-api/actions/workflows/ci.yml) [![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE) [![MCP Registry](https://img.shields.io/badge/MCP_Registry-io.github.NITI--Lab%2Fajiro--api-blue)](https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.NITI-Lab/ajiro-api) [![NITI-Lab/ajiro-api MCP server](https://glama.ai/mcp/servers/NITI-Lab/ajiro-api/badges/score.svg)](https://glama.ai/mcp/servers/NITI-Lab/ajiro-api)

English | [日本語](README.ja.md)

Ajiro API (`ajiro-api`) is a CLI that statically scans your backend routes into one YAML file per endpoint, runs those files as regression tests, and serves them as mocks. It is built so that Claude Code and Codex can run "implement -> scan -> test -> fix -> re-test" with no human in the loop.

![Demo: ajiro-api scan and test pass, a moved route makes GET /todos/stats return 404, the fix makes all 34 tests pass again](docs/demo/demo.gif)

With `--json`, the same failure comes with the request, the response body and the handler location, so the agent can fix it and re-run on its own. In Claude Code:

```text
/plugin marketplace add niti-lab/ajiro-api
/plugin install ajiro-api@ajiro-api
```

Supports Next.js Route Handlers, Hono, FastAPI, Express and Go (net/http, chi, Echo).

- Local-first: API definitions and test results never leave your machine.
- Plain YAML, one file per endpoint: fewer Git conflicts, and a narrow surface for AI to edit.
- Single Rust binary: no Node or Python needed at runtime.
- MIT licensed: the CLI, skill and test runner are free.

A sister product of Ajiro (WBS / task management).

## Why

- When AI implements APIs, checking every endpoint and re-checking after each change stays manual.
- Existing HTTP clients assume a human sending requests from a GUI, not an agent reading and writing files.
- In contract work, API specs and sample responses always have to be handed to frontend developers and clients.

## 30-second quickstart

### Install

```bash
# npm (run without installing)
npx -y ajiro-api --version

# Homebrew
brew install niti-lab/tap/ajiro-api

# cargo
cargo install ajiro-api

# GitHub Releases (macOS / Linux / Windows binaries)
# https://github.com/niti-lab/ajiro-api/releases
```

If `ajiro-api` is not on your PATH, replace it with `npx -y ajiro-api` in the commands below.

### Use

```bash
cd your-backend-project
ajiro-api init            # creates ajiro-api.yaml and api/ (framework auto-detected)
ajiro-api scan            # reads routes and writes YAML into api/
# start your app, then
ajiro-api test            # runs every endpoint and checks the responses
```

Endpoints with empty `tests:` get an implicit `smoke` test (status < 500; a 4xx passes with a warning, since a 401 usually means the run is not logged in). It is only sent to GET, HEAD and OPTIONS; POST, PUT, PATCH and DELETE may change data and are skipped unless you pass `ajiro-api test --smoke-unsafe`. For APIs behind a login, list the login endpoint in the environment's `setup`: it runs first, and the token or cookie it captures can be sent with every request through the environment `headers` (see the [YAML reference](docs/yaml-reference.md)).

## YAML example

One YAML file per endpoint, at `api/<resource>/<method>-<path>.yaml`. `scan` writes `source`; humans and AI write `tests` and `mock`. `scan` never rewrites anything except `source`.

```yaml
# api/users/post-users.yaml
version: 1
id: users.create
request:
  method: POST
  path: /users
  headers:
    Authorization: Bearer {{ auth.token }}
  body:
    name: Taro
    email: taro@example.com
source:            # updated by scan
  framework: hono
  file: src/routes/users.ts
  line: 42
tests:             # written by humans and AI (scan never touches it)
  - name: creates a user
    expect:
      status: 201
      body:
        schema: ./schemas/user.json
        match:
          $.email: taro@example.com
      ignore: [$.id, $.createdAt]
    capture:
      userId: $.id
  - name: duplicate email returns 409
    request:
      body: { email: dup@example.com }
    expect:
      status: 409
mock:              # used by `ajiro-api mock`
  default: { status: 201, body: { id: usr_001, name: Taro } }
  scenarios:
    conflict: { status: 409 }
```

See [docs/yaml-reference.md](docs/yaml-reference.md) (Japanese) and `skills/ajiro-api/reference.md` (English) for the full syntax. The JSON Schema is available via `ajiro-api schema endpoint` and works for editor completion:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/niti-lab/ajiro-api/main/schema/endpoint.schema.json
```

## Claude Code

Install the plugin to get the skill (the scan -> test -> fix -> re-run procedure) and the MCP server:

```text
/plugin marketplace add niti-lab/ajiro-api
/plugin install ajiro-api@ajiro-api
```

The skill activates when the agent implements or changes HTTP API endpoints, or when you ask it to test or verify an API. See [skills/ajiro-api/SKILL.md](skills/ajiro-api/SKILL.md).

The MCP server is listed in the [official MCP Registry](https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.NITI-Lab/ajiro-api) as `io.github.NITI-Lab/ajiro-api` and on [Glama](https://glama.ai/mcp/servers/NITI-Lab/ajiro-api), so any MCP client can use it. To use only the MCP server, add this to `.mcp.json` (or your client's MCP settings):

```json
{
  "mcpServers": {
    "ajiro-api": { "command": "npx", "args": ["-y", "ajiro-api", "mcp"] }
  }
}
```

Or, in Claude Code:

```bash
claude mcp add ajiro-api -- npx -y ajiro-api mcp
```

Tools: `scan`, `test`, `list`, `validate`, `get_endpoint`, `schema`.

## Codex

Paste [docs/agents/AGENTS.snippet.md](docs/agents/AGENTS.snippet.md) into your project's `AGENTS.md`. It is the same procedure as the Claude Code skill.

## Commands

| Command | Description |
| --- | --- |
| `ajiro-api init` | Create `ajiro-api.yaml` and `api/`; detect the framework |
| `ajiro-api scan` | Statically analyse routes and write/update YAML. `--check` for CI drift detection (add `--ignore-lines` to tolerate routes that only moved), `--dry-run` to preview |
| `ajiro-api test` | Run the YAML and check responses. `--json`, `--junit <FILE>`, `-u` (update snapshots), `--env`, `--bail`, ... |
| `ajiro-api list` | List defined endpoints |
| `ajiro-api validate` | Validate syntax, schema and references (duplicate ids, `depends_on`) |
| `ajiro-api mock` | Serve the `mock:` responses (default port 4010) |
| `ajiro-api mcp` | Run the MCP server over stdio |
| `ajiro-api openapi import / export` | Convert from/to OpenAPI 3.x |
| `ajiro-api import postman / bruno` | Import Postman collections or Bruno folders |
| `ajiro-api docs` | Generate a single-file HTML API reference |
| `ajiro-api schema [endpoint\|config]` | Print the JSON Schema |
| `ajiro-api telemetry [on\|off\|status]` | Usage statistics consent (off by default) |

All flags: [docs/cli-reference.md](docs/cli-reference.md) (Japanese) or `ajiro-api <command> --help`. Exit codes: `0` success, `1` tests failed / drift / validation errors, `2` the tool itself could not run (config or I/O error).

`--json` output contains the failing check, expected value, actual value and the source location, so an agent can fix the problem directly. Tokens and passwords are masked as `***`.

## Supported frameworks

| Framework | `framework` value |
| --- | --- |
| Next.js Route Handlers | `nextjs` |
| Hono | `hono` |
| FastAPI | `fastapi` |
| Express | `express` |
| Go net/http | `go-nethttp` |
| Go chi | `go-chi` |
| Go Echo | `go-echo` |

GraphQL, gRPC and WebSocket are out of scope.

## CI

```bash
ajiro-api scan --check                          # exit 1 if YAML is out of date
ajiro-api scan --check --ignore-lines           # ...but not when routes only moved to another line
ajiro-api test --env ci --junit report.xml      # also writes JUnit XML
```

A GitHub Actions sample is in [examples/github-actions/ajiro-api.yml](examples/github-actions/ajiro-api.yml): install, start the app in the background, wait for the port, `scan --check`, `test`, publish JUnit, upload artifacts.

## OpenAPI, Postman and Bruno

```bash
ajiro-api openapi import openapi.yaml           # OpenAPI 3.x -> YAML (tests are kept)
ajiro-api openapi export -o openapi.yaml        # YAML -> OpenAPI 3.1
ajiro-api import postman collection.json        # Postman Collection v2.1
ajiro-api import bruno ./bruno-collection       # Bruno .bru folder
```

Add `--overwrite` to overwrite `request` and `mock` of existing files. `tests` are always kept.

## Mock

```bash
ajiro-api mock                      # http://127.0.0.1:4010
ajiro-api mock --scenario conflict  # pick a scenario at startup
```

Switch scenarios per request with the `X-Ajiro-Scenario` header or the `?__scenario=` query parameter. Endpoints without that scenario return `default`. `--delay` adds latency to every response.

## HTML docs

```bash
ajiro-api docs -o api-docs.html --title "My API"
```

A single file that opens in any browser, so you can hand it to people who cannot set up Swagger.

## Comparison

Our axes: AI agents as the primary user, plain standard YAML instead of a proprietary format, and no per-seat pricing. Prices are from the vendors' official pages as of November 2026 (annual billing).

| Product | Free tier | Team pricing | Storage format |
| --- | --- | --- | --- |
| [Postman](https://www.postman.com/pricing/) | Single user only; no team creation (changed March 2026) | Team $19/user/month | Cloud-centric, Collection JSON |
| [Bruno](https://usebruno.com/pricing) | OSS edition; public Git provider integration | Pro $6 / Ultimate $11 per user/month | Proprietary `.bru` |
| **Ajiro API** | CLI, skill and test runner are free (MIT) | CLI is free | Standard YAML (convertible to/from OpenAPI) |

Postman and Bruno are good tools for humans building requests in a GUI, and Ajiro API does not compete there. It focuses on YAML generated from code, read and written by AI, and run continuously in CI. `ajiro-api import postman` and `ajiro-api import bruno` help you migrate.

## Privacy

- Local-first: API definitions and test results are never sent to a server.
- Usage statistics are off by default. Only if you run `ajiro-api telemetry on` are anonymous command counts (command name, version, OS) kept; no paths, URLs or API definitions. `DO_NOT_TRACK=1` always disables it.
- Secrets are referenced from `.env` only (`{{ env.NAME }}`), never written in YAML. Tokens and passwords are masked as `***` in output.

## Roadmap

- **Phase 1 (now)**: OSS CLI, Claude Code skill, Codex instructions, MCP server.
- **Phase 2**: a paid desktop GUI (full mock server, test result views, ...). Whether to build it is decided after we see usage after launch; no dates are promised.
- **Phase 3**: task integration with Ajiro (WBS / task management).

The CLI, skill, MCP server and importers stay MIT and free in Phase 2 and later.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). We also track "GUI / mock / spec requests" via an issue label. Security reports: [SECURITY.md](SECURITY.md). Changes: [CHANGELOG.md](CHANGELOG.md).

## License

[MIT](LICENSE) (c) 2026 NITI
