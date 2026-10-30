"""Monty's `annotationlib`: `Format`, `ForwardRef` and `get_annotations()`, without the
`__annotate__` helpers (`call_annotate_function`, `annotations_to_string`, ...).
`Format` is an `int` subclass here rather than an `IntEnum`, since Monty has no `enum`
module and its members are plain ints; see `limitations/annotationlib.md`."""

import types
from collections.abc import Mapping
from typing import Any, final

__all__ = ['Format', 'ForwardRef', 'get_annotations']

class Format(int):
    VALUE: Format
    VALUE_WITH_FAKE_GLOBALS: Format
    FORWARDREF: Format
    STRING: Format

@final
class ForwardRef:
    __forward_is_argument__: bool
    __forward_is_class__: bool
    __forward_module__: str | None
    __owner__: object
    def __init__(
        self,
        arg: str,
        *,
        module: str | None = None,
        owner: object = None,
        is_argument: bool = True,
        is_class: bool = False,
    ) -> None: ...
    def evaluate(
        self,
        *,
        globals: dict[str, Any] | None = None,
        locals: Mapping[str, Any] | None = None,
        type_params: tuple[Any, ...] | None = None,
        owner: object = None,
        format: Format = ...,
    ) -> Any: ...
    @property
    def __forward_arg__(self) -> str: ...
    @property
    def __resolved_str__(self) -> str: ...
    def __eq__(self, other: object) -> bool: ...
    def __hash__(self) -> int: ...
    def __or__(self, other: Any) -> types.UnionType: ...
    def __ror__(self, other: Any) -> types.UnionType: ...

def get_annotations(
    obj: Any,  # any object with __annotations__ or __annotate__
    *,
    globals: dict[str, object] | None = None,
    locals: Mapping[str, object] | None = None,
    eval_str: bool = False,
    format: Format = ...,
) -> dict[str, Any]: ...
