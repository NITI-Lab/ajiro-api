from fastapi import FastAPI

from app.routers import items

app = FastAPI(title="fastapi-items")

app.include_router(items.router)


@app.get("/health")
def health() -> dict[str, str]:
    return {"status": "ok"}
