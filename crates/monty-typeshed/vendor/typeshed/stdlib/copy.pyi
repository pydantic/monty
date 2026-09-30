# Based on python/typeshed's `stdlib/copy.pyi`, cut down to what Monty
# implements at runtime. `replace()` and the `Error` / `error` class are absent
# so type checking rejects them up front instead of letting them fail with
# `AttributeError`. Extend in lockstep with `crates/monty/src/modules/copy.rs`.

from typing import Any, TypeVar

__all__ = ['copy', 'deepcopy']

_T = TypeVar('_T')

def copy(x: _T) -> _T: ...
def deepcopy(x: _T, memo: dict[int, Any] | None = None, _nil: Any = []) -> _T: ...
