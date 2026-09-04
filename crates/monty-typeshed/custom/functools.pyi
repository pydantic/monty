# Based on python/typeshed's `stdlib/functools.pyi`, stripped down to the
# callables Monty actually implements at runtime.
#
# Everything else (`wraps`, `partialmethod`, `cached_property`,
# `total_ordering`, `singledispatch`, `cmp_to_key`, `Placeholder`, …) is
# deliberately absent so type checking rejects it up front, rather than passing
# and then raising `AttributeError` at runtime.
# Extend this in lockstep with `crates/monty/src/modules/functools.rs`.
#
# `partial` keeps upstream's `Generic[_T]` so `partial[...]` stays usable as an
# annotation, which runs fine — Monty stringizes annotations rather than
# evaluating them. It is not what types a call: ty models `partial` itself and
# infers `partial[() -> int]` with or without the `Generic` base.
#
# The cost of keeping it is that `partial[int]` as a runtime *expression* type
# checks and then raises `TypeError: 'type' object is not subscriptable`,
# exactly as `list[int]` does — Monty has no runtime generic aliases at all
# (see limitations/typing.md), so no stub can close that gap. Dropping
# `Generic` would only trade it for rejecting the annotation form, which works.
# Upstream's `__class_getitem__` is dropped as dead weight, not as a guard:
# `Generic` leaves the class subscriptable at check time either way.
#
# `cache_parameters()` is a plain dict rather than upstream's TypedDict, and
# `CacheInfo` a tuple subclass rather than a NamedTuple, so both stay within the
# stub surface Monty's checker reads.

from collections.abc import Callable, Hashable, Iterable
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

class CacheInfo(tuple[int, int, int | None, int]):
    @property
    def hits(self) -> int: ...
    @property
    def misses(self) -> int: ...
    @property
    def maxsize(self) -> int | None: ...
    @property
    def currsize(self) -> int: ...

class _lru_cache_wrapper(Generic[_T]):
    __wrapped__: Callable[..., _T]
    def __call__(self, *args: Hashable, **kwargs: Hashable) -> _T: ...
    def cache_info(self) -> CacheInfo: ...
    def cache_clear(self) -> None: ...
    def cache_parameters(self) -> dict[str, Any]: ...

@overload
def lru_cache(
    maxsize: int | None = 128, typed: bool = False
) -> Callable[[Callable[..., _T]], _lru_cache_wrapper[_T]]: ...
@overload
def lru_cache(maxsize: Callable[..., _T], typed: bool = False) -> _lru_cache_wrapper[_T]: ...
def cache(user_function: Callable[..., _T], /) -> _lru_cache_wrapper[_T]: ...
