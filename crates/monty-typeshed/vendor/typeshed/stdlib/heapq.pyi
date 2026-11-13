"""Monty's `heapq`: `_heapq`'s functions plus `nlargest` and `nsmallest`. `merge()` is not available."""

from _heapq import (
    heapify as heapify,
    heapify_max as heapify_max,
    heappop as heappop,
    heappop_max as heappop_max,
    heappush as heappush,
    heappush_max as heappush_max,
    heappushpop as heappushpop,
    heappushpop_max as heappushpop_max,
    heapreplace as heapreplace,
    heapreplace_max as heapreplace_max,
)
from collections.abc import Callable, Iterable
from typing import SupportsIndex, TypeVar

from _typeshed import SupportsRichComparison

_S = TypeVar('_S')

def nlargest(
    n: SupportsIndex, iterable: Iterable[_S], key: Callable[[_S], SupportsRichComparison] | None = None
) -> list[_S]: ...
def nsmallest(
    n: SupportsIndex, iterable: Iterable[_S], key: Callable[[_S], SupportsRichComparison] | None = None
) -> list[_S]: ...
