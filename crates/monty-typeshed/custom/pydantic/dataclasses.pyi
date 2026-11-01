# Monty's `pydantic.dataclasses` subset: the validating `dataclass` decorator.
# See docs/limitations/pydantic.md for what it accepts at runtime.
from collections.abc import Callable
from typing import Any, TypeVar, overload

from pydantic import ConfigDict, Field as Field

_T = TypeVar('_T')

@overload
def dataclass(
    _cls: None = None,
    *,
    init: bool = False,
    repr: bool = True,
    eq: bool = True,
    order: bool = False,
    unsafe_hash: bool = False,
    frozen: bool | None = None,
    config: ConfigDict | None = None,
    validate_on_init: bool | None = None,
    kw_only: bool = False,
    slots: bool = False,
) -> Callable[[type[_T]], type[_T]]: ...
@overload
def dataclass(
    _cls: type[_T],
    /,
    *,
    init: bool = False,
    repr: bool = True,
    eq: bool = True,
    order: bool = False,
    unsafe_hash: bool = False,
    frozen: bool | None = None,
    config: ConfigDict | None = None,
    validate_on_init: bool | None = None,
    kw_only: bool = False,
    slots: bool = False,
) -> type[_T]: ...
def is_pydantic_dataclass(obj: Any, /) -> bool: ...
