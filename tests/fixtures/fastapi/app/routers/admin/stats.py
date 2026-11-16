from fastapi import APIRouter

router = APIRouter()


@router.options("/stats")
async def stats_options(window: str = "7d"):
    return {}


@router.get("/stats/{name}")
async def stats(name: str, since: float = 0.0, *, flag: bool = True):
    return {}
