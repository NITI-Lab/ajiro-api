from typing import Annotated, Optional

from fastapi import APIRouter, Depends, Header, Query, Request

from ..models import Role, UserIn
from ..deps import get_db

router = APIRouter(prefix="/users", tags=["users"])


@router.get("/")
async def list_users(
    request: Request,
    limit: int = 10,
    offset: Optional[int] = None,
    role: Role | None = None,
    tags: list[str] = Query(default=[]),
    db=Depends(get_db),
    x_token: str = Header(None),
):
    return []


@router.post("", status_code=201)
def create_user(body: UserIn, dry_run: bool = False):
    return body


@router.get("/{user_id}")
def get_user(user_id: int, verbose: Annotated[bool, Query()] = False, q: "str" = ""):
    return {}


@router.put(path="/{user_id:int}")
def replace_user(user_id: int, body: UserIn):
    return body


@router.delete("/{user_id}")
def delete_user(user_id: int, force: bool = False):
    return None
