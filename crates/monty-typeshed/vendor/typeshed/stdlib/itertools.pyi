# Based on python/typeshed's `stdlib/itertools.pyi`, stripped down to the
# callables Monty actually implements at runtime.
#
# Everything else in `itertools` (`chain`, `islice`, `cycle`, `product`, …) is
# deliberately absent so type checking rejects it up front, rather than passing
# and then raising `AttributeError` at runtime. Extend this in lockstep with
# `crates/monty/src/modules/itertools.rs`.
#
# Narrower than upstream on purpose: `_N` is constrained to `int`/`float`
# because Monty's `count()` rejects anything else, where CPython accepts any
# number protocol.

from typing import Generic, TypeAlias, TypeVar, overload

from typing_extensions import Self

_T = TypeVar('_T')
_N = TypeVar('_N', int, float)

_Step: TypeAlias = int | float

class count(Generic[_N]):
    @overload
    def __new__(cls) -> count[int]: ...
    @overload
    def __new__(cls, start: _N, step: _Step = 1) -> count[_N]: ...
    @overload
    def __new__(cls, *, step: _N) -> count[_N]: ...
    def __next__(self) -> _N: ...
    def __iter__(self) -> Self: ...

class repeat(Generic[_T]):
    @overload
    def __new__(cls, object: _T) -> Self: ...
    @overload
    def __new__(cls, object: _T, times: int) -> Self: ...
    def __next__(self) -> _T: ...
    def __iter__(self) -> Self: ...
