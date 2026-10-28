# Based on python/typeshed's `stdlib/copy.pyi`, cut down to what Monty
# implements at runtime. The `Error` / `error` class is absent so type checking
# rejects it up front instead of letting it fail with `AttributeError`. Extend
# in lockstep with `crates/monty/src/modules/copy.rs`.

from typing import Any, Protocol, TypeVar

from typing_extensions import Self

__all__ = ['copy', 'deepcopy', 'replace']

_T = TypeVar('_T')
_SR = TypeVar('_SR', bound=_SupportsReplace)

class _SupportsReplace(Protocol):
    # In reality doesn't support args, but there's no other great way to express this.
    def __replace__(self, *args: Any, **kwargs: Any) -> Self: ...

def copy(x: _T) -> _T: ...
def deepcopy(x: _T, memo: dict[int, Any] | None = None, _nil: Any = []) -> _T: ...
def replace(obj: _SR, /, **changes: Any) -> _SR: ...
