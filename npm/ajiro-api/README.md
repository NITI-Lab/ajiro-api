# ajiro-api

API definitions, regression tests and mocks for AI coding agents. Scans your backend routes (Next.js, Hono, FastAPI, Express, Go) into one YAML file per endpoint, runs them as tests, and serves them as mocks.

```bash
npx -y ajiro-api init
npx -y ajiro-api scan
npx -y ajiro-api test
```

This package is a small launcher. The native binary comes from the platform package (`@ajiro-api/cli-<platform>-<arch>`) installed automatically as an optional dependency. Supported: Linux x64/arm64, macOS x64/arm64, Windows x64. Elsewhere use `cargo install ajiro-api` or the [GitHub Releases](https://github.com/niti-lab/ajiro-api/releases).

Documentation: https://github.com/niti-lab/ajiro-api

MIT License.
