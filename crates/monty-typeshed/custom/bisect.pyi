from collections.abc import Callable, MutableSequence
from typing import SupportsIndex, TypeVar, overload

from _typeshed import SupportsLenAndGetItem, SupportsRichComparison, SupportsRichComparisonT

_T = TypeVar('_T')

@overload
def bisect_left(
    a: SupportsLenAndGetItem[SupportsRichComparisonT],
    x: SupportsRichComparisonT,
    lo: SupportsIndex = 0,
    hi: SupportsIndex | None = None,
    *,
    key: None = None,
) -> int: ...
@overload
def bisect_left(
    a: SupportsLenAndGetItem[_T],
    x: SupportsRichComparisonT,
    lo: SupportsIndex = 0,
    hi: SupportsIndex | None = None,
    *,
    key: Callable[[_T], SupportsRichComparisonT],
) -> int: ...
@overload
def insort_left(
    a: MutableSequence[SupportsRichComparisonT],
    x: SupportsRichComparisonT,
    lo: SupportsIndex = 0,
    hi: SupportsIndex | None = None,
    *,
    key: None = None,
) -> None: ...
@overload
def insort_left(
    a: MutableSequence[_T],
    x: _T,
    lo: SupportsIndex = 0,
    hi: SupportsIndex | None = None,
    *,
    key: Callable[[_T], SupportsRichComparison],
) -> None: ...

bisect_right = bisect_left
bisect = bisect_right
insort_right = insort_left
insort = insort_right
