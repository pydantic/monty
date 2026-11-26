# Based on python/typeshed's `stdlib/functools.pyi`, stripped down to the
# callables Monty actually implements at runtime.
#
# Everything else (`wraps`, `partialmethod`, `cached_property`,
# `total_ordering`, `singledispatch`, `cmp_to_key`, `Placeholder`, …) is
# deliberately absent so type checking rejects it up front, rather than passing
# and then raising `AttributeError` at runtime.
# Extend this in lockstep with `crates/monty/src/modules/functools.rs`.
#
# `partial` keeps upstream's `Generic[_T]`: `partial[int]` builds a generic
# alias at runtime (see limitations/typing.md).
#
# The cache wrapper uses a `ParamSpec` instead of upstream's `*args: Hashable`
# to preserve the wrapped signature. If the signature accepts an unhashable
# argument, the call can pass type checking and still raise `TypeError`.

from collections.abc import Callable, Iterable
from typing import Any, Generic, ParamSpec, TypeVar, overload

from _typeshed._monty import _lru_cache_wrapper
from typing_extensions import Self

_T = TypeVar('_T')
_P = ParamSpec('_P')
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

@overload
def lru_cache(
    maxsize: int | None = 128, typed: bool = False
) -> Callable[[Callable[_P, _T]], _lru_cache_wrapper[_P, _T]]: ...
@overload
def lru_cache(maxsize: Callable[_P, _T], typed: bool = False) -> _lru_cache_wrapper[_P, _T]: ...
def cache(user_function: Callable[_P, _T], /) -> _lru_cache_wrapper[_P, _T]: ...
