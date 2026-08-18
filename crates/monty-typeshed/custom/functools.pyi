# Based on python/typeshed's `stdlib/functools.pyi`, stripped down to the
# callables Monty actually implements at runtime.
#
# Everything else (`partial`, `cache`, `lru_cache`, `wraps`, `partialmethod`,
# `cached_property`, `total_ordering`, `singledispatch`, `cmp_to_key`,
# `Placeholder`, ...) is deliberately absent so type checking rejects it up
# front, rather than passing and then raising `AttributeError` at runtime.
# Extend this in lockstep with `crates/monty/src/modules/functools.rs`.

from collections.abc import Callable, Iterable
from typing import TypeVar, overload

_T = TypeVar('_T')
_S = TypeVar('_S')

@overload
def reduce(function: Callable[[_T, _S], _T], iterable: Iterable[_S], /, initial: _T) -> _T: ...
@overload
def reduce(function: Callable[[_T, _T], _T], iterable: Iterable[_T], /) -> _T: ...
