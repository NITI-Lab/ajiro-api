from fastapi import FastAPI
from app.routers import users, items
from app.routers.admin import stats as admin_stats
from . import config
from .routers.orphan import router as orphan_router
import app.routers.items as items_mod

app = FastAPI(title="shop")

app.include_router(users.router, prefix=config.V1)
app.include_router(items.router, prefix="/api")
app.include_router(admin_stats.router, prefix="/admin")


@app.get("/health")
def health():
    return {"ok": True}


@app.get("/")
async def root():
    return {}


@app.api_route("/echo", methods=["GET", "POST"])
async def echo(msg: str = "hi"):
    return msg


app.add_api_route("/ping", health, methods=["GET"])
