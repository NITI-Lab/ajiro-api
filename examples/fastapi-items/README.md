# fastapi-items

A tiny FastAPI app (a router mounted with `include_router`) used as a scan fixture and demo for ajiro-api.

```bash
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
uvicorn app.main:app --port 8000
```

Then, in another terminal:

```bash
npx -y ajiro-api init --framework fastapi --base-url http://localhost:8000
npx -y ajiro-api scan
npx -y ajiro-api test
```

Endpoints: `GET /health`, `GET /items`, `POST /items` (201, 409 on duplicate name), `GET /items/{item_id}`, `DELETE /items/{item_id}` (204).
