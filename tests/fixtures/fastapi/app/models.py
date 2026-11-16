from enum import Enum

from pydantic import BaseModel


class Role(str, Enum):
    admin = "admin"
    user = "user"


class UserIn(BaseModel):
    name: str
    email: str
