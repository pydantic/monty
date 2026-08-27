# Based on python/typeshed's `stdlib/functools.pyi`, stripped down to the
# callables Monty actually implements at runtime.
#
# Everything else (`cache`, `lru_cache`, `wraps`, `partialmethod`,
# `cached_property`, `total_ordering`, `singledispatch`, `cmp_to_key`,
# `Placeholder`, …) is deliberately absent so type checking rejects it up
# front, rather than passing and then raising `AttributeError` at runtime.
# Extend this in lockstep with `crates/monty/src/modules/functools.rs`.
#
# `partial` drops the `__class_getitem__` upstream declares, since Monty has no
# subscriptable type objects.

from collections.abc import Callable, Iterable
from typing import Any, Generic, TypeVar, overload

from typing_extensions import Self

_T = TypeVar('_T')
_S = TypeVar('_S')

@overload
def reduce(function: Callable[[_T, _S], _T], iterable: Iterable[_S], /, initial: _T) -> _T: ...
@overload
def reduce(function: Callable[[_T, _T], _T], iterable: Iterable[_T], /) -> _T: ...

class partial(Generic[_T]):
    @property
    def func(self) -> Callable[..., _T]: ...
    @property
    def args(self) -> tuple[Any, ...]: ...
    @property
    def keywords(self) -> dict[str, Any]: ...
    def __new__(cls, func: Callable[..., _T], /, *args: Any, **kwargs: Any) -> Self: ...
    def __call__(self, /, *args: Any, **kwargs: Any) -> _T: ...
