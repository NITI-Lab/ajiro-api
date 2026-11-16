# hono-todo

A small Hono app (TypeScript, `@hono/node-server`, run with `tsx`) used to demo the ajiro-api agent loop: an AI agent implements an endpoint, then scans, tests and self-fixes without a human.

## Endpoints

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/health` | no auth |
| POST | `/auth/login` | body `{ "username": "demo", "password": "demo-password" }` (override with `DEMO_PASSWORD`), returns `{ "token": "..." }` |
| GET | `/todos` | bearer token; optional `?done=true\|false` |
| POST | `/todos` | bearer token; body `{ "title": "..." }`; 400 on invalid title, 409 on duplicate title |
| GET | `/todos/:id` | bearer token; 404 if missing |
| PATCH | `/todos/:id` | bearer token; body `{ "title"?, "done"? }`; 400, 404, 409 |
| DELETE | `/todos/:id` | bearer token; 204, or 404 |

Routes are split across `src/routes/auth.ts` and `src/routes/todos.ts` and mounted with `app.route()` in `src/index.ts`. Data is kept in memory (`src/store.ts`).

## Run

```bash
npm install
npm run dev      # watch mode, http://localhost:3000
npm start        # no watch
```

## The agent demo

```bash
npx -y ajiro-api init     # detects Hono, writes ajiro-api.yaml and api/
npx -y ajiro-api scan     # one YAML file per endpoint under api/
npm start &               # start the app in the background
npx -y ajiro-api test --json
```

Then ask your agent (with the ajiro-api plugin or `AGENTS.md` snippet installed), for example:

> Add `GET /todos/stats` returning `{ total, done }` (auth required). Then verify the API.

The agent should implement the route, run `ajiro-api scan --json`, write tests for the new endpoint (and the auth chain with `depends_on: [auth.login.create]` and `capture`), run `ajiro-api test --json`, fix whatever fails, and re-run until green. Put the demo password in `.env` (`DEMO_PASSWORD=demo-password`) and reference it as `{{ env.DEMO_PASSWORD }}` in the login test; never write it in YAML.
