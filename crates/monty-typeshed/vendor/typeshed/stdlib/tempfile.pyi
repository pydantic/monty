"""Monty's named-file subset; storage is supplied through host mounts or OS callbacks."""

from os import PathLike
from typing import IO, Any, Literal, overload

@overload
def NamedTemporaryFile(
    mode: Literal['w', 'a', 'wt', 'at'],
    buffering: int = -1,
    encoding: str | None = None,
    newline: str | None = None,
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | PathLike[str] | None = None,
    delete: bool = True,
    *,
    errors: str | None = None,
    delete_on_close: bool = True,
) -> IO[str]: ...
@overload
def NamedTemporaryFile(
    mode: Literal['wb', 'ab', 'w+b', 'wb+'] = 'w+b',
    buffering: int = -1,
    encoding: str | None = None,
    newline: str | None = None,
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | PathLike[str] | None = None,
    delete: bool = True,
    *,
    errors: str | None = None,
    delete_on_close: bool = True,
) -> IO[bytes]: ...
@overload
def NamedTemporaryFile(
    mode: str = 'w+b',
    buffering: int = -1,
    encoding: str | None = None,
    newline: str | None = None,
    suffix: str | None = None,
    prefix: str | None = None,
    dir: str | PathLike[str] | None = None,
    delete: bool = True,
    *,
    errors: str | None = None,
    delete_on_close: bool = True,
) -> IO[Any]: ...
def gettempdir() -> str: ...
def gettempprefix() -> str: ...
