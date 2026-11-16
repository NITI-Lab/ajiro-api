from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

router = APIRouter(prefix="/items", tags=["items"])


class ItemIn(BaseModel):
    name: str = Field(min_length=1, max_length=100)
    price: float = Field(ge=0)


class Item(ItemIn):
    id: int


_items: dict[int, Item] = {}
_next_id = 1


@router.get("")
def list_items() -> list[Item]:
    return list(_items.values())


@router.post("", status_code=201)
def create_item(body: ItemIn) -> Item:
    global _next_id
    if any(i.name == body.name for i in _items.values()):
        raise HTTPException(status_code=409, detail="Item already exists")
    item = Item(id=_next_id, **body.model_dump())
    _items[item.id] = item
    _next_id += 1
    return item


@router.get("/{item_id}")
def get_item(item_id: int) -> Item:
    item = _items.get(item_id)
    if item is None:
        raise HTTPException(status_code=404, detail="Item not found")
    return item


@router.delete("/{item_id}", status_code=204)
def delete_item(item_id: int) -> None:
    if _items.pop(item_id, None) is None:
        raise HTTPException(status_code=404, detail="Item not found")
