from fastapi import APIRouter

router = APIRouter(prefix="/orphans")


@router.head("/{oid}")
def head_orphan(oid: str):
    pass


@router.patch("/{oid}")
def patch_orphan(oid: str, note: str):
    pass


@router.get("/files/{file_path:path}")
def get_file(file_path: str):
    pass


@router.get("/dyn/" + SOMETHING)
def dyn():
    pass
