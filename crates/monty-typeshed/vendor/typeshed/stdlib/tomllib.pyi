# Modified from python/typeshed's stdlib/tomllib.pyi to exclude parse_float.
from typing import Any, BinaryIO

__all__ = ('loads', 'load', 'TOMLDecodeError')

# Inside the sandbox the exception is message-only: the `msg` / `doc` / `pos` /
# `lineno` / `colno` attributes CPython 3.14 adds are not available.
class TOMLDecodeError(ValueError): ...

# `parse_float` is not implemented, see limitations/tomllib.md.
def load(fp: BinaryIO, /) -> dict[str, Any]: ...
def loads(s: str, /) -> dict[str, Any]: ...
