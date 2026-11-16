from fastapi import APIRouter

router = APIRouter(prefix="/items")
sub = APIRouter(prefix="/{item_id}/reviews")

router.include_router(sub, prefix="/v2")


@router.get("/")
def list_items(page: int = 1, size: int = 20):
    return []


@sub.get("/")
def list_reviews(item_id: int, stars: int | None = None):
    return []


@sub.post("/")
def add_review(item_id: int):
    return {}
