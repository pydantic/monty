from typing import Any

from _typeshed import SupportsRead

__all__ = ('loads', 'load', 'TOMLDecodeError')

# Inside the sandbox the exception is message-only: the `msg` / `doc` / `pos` /
# `lineno` / `colno` attributes CPython 3.14 adds are not available.
class TOMLDecodeError(ValueError): ...

# `parse_float` is not implemented, see limitations/tomllib.md. The runtime
# accepts only file objects from `open(..., 'rb')`, a subset of this protocol.
def load(fp: SupportsRead[bytes], /) -> dict[str, Any]: ...
def loads(s: str, /) -> dict[str, Any]: ...
