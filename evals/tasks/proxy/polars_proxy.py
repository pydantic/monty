"""Analyse a DataFrame through a host-proxied, polars-shaped API.

`Frame` stands in for a polars DataFrame: every method returns a new `Frame` or a
`GroupBy`, and the sandbox only ever holds a `ClassInstance` proxy. Chaining works
because `FrameProxy.convert_value` wraps each returned host object in another proxy,
and `height` is a lazy attribute served on demand. The methods are sync, so the code
calls them without `await` and each call is a host round trip.
"""

from __future__ import annotations

import operator
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.task import Task
from pydantic_monty import ClassInstance

_OPS: dict[str, Callable[[Any, Any], bool]] = {
    '==': operator.eq,
    '!=': operator.ne,
    '<': operator.lt,
    '<=': operator.le,
    '>': operator.gt,
    '>=': operator.ge,
}


@dataclass
class Frame:
    """A list of rows with a polars-like fluent API; every method returns a new frame."""

    rows: list[dict[str, Any]] = field(default_factory=list)

    @property
    def height(self) -> int:
        return len(self.rows)

    def filter(self, col: str, op: str, value: Any) -> Frame:
        """Rows where `col <op> value`; `op` is one of `==`, `!=`, `<`, `<=`, `>`, `>=`."""
        if op not in _OPS:
            raise ValueError(f'unknown operator {op!r}')
        return Frame([r for r in self.rows if _OPS[op](r[col], value)])

    def select(self, cols: list[str]) -> Frame:
        return Frame([{c: r[c] for c in cols} for r in self.rows])

    def sort(self, col: str, descending: bool = False) -> Frame:
        return Frame(sorted(self.rows, key=lambda r: r[col], reverse=descending))

    def head(self, n: int) -> Frame:
        return Frame(self.rows[:n])

    def group_by(self, col: str) -> GroupBy:
        return GroupBy(self, col)

    def to_dicts(self) -> list[dict[str, Any]]:
        return [dict(r) for r in self.rows]


@dataclass
class GroupBy:
    """The pending grouping `Frame.group_by` returns; `agg` produces the grouped frame."""

    frame: Frame
    col: str

    def agg(self, sum_of: str | None = None, count: bool = False) -> Frame:
        """One row per group with `<sum_of>_sum` and/or `count` columns, in first-seen order."""
        groups: dict[Any, dict[str, Any]] = {}
        for row in self.frame.rows:
            key = row[self.col]
            out = groups.setdefault(key, {self.col: key})
            if sum_of is not None:
                out[f'{sum_of}_sum'] = round(out.get(f'{sum_of}_sum', 0) + row[sum_of], 2)
            if count:
                out['count'] = out.get('count', 0) + 1
        return Frame(list(groups.values()))


class FrameProxy(ClassInstance):
    """`ClassInstance` that keeps chained results proxied instead of failing conversion."""

    def convert_value(self, /, name: str, value: Any) -> Any:
        if isinstance(value, (Frame, GroupBy)):
            return FrameProxy(value, allowed_methods='all', lazy_attrs={'height'} if isinstance(value, Frame) else None)
        return value


_SALES = [
    {'region': 'EMEA', 'product': 'widget', 'amount': 1200.0, 'units': 12, 'month': '2026-07'},
    {'region': 'AMER', 'product': 'gadget', 'amount': 900.5, 'units': 3, 'month': '2026-07'},
    {'region': 'EMEA', 'product': 'gadget', 'amount': 450.25, 'units': 5, 'month': '2026-08'},
    {'region': 'APAC', 'product': 'widget', 'amount': 2100.0, 'units': 21, 'month': '2026-08'},
    {'region': 'EMEA', 'product': 'widget', 'amount': 300.75, 'units': 3, 'month': '2026-08'},
    {'region': 'AMER', 'product': 'widget', 'amount': 1500.0, 'units': 15, 'month': '2026-09'},
    {'region': 'APAC', 'product': 'gadget', 'amount': 725.5, 'units': 7, 'month': '2026-09'},
    {'region': 'AMER', 'product': 'gadget', 'amount': 610.0, 'units': 2, 'month': '2026-09'},
    {'region': 'EMEA', 'product': 'widget', 'amount': 880.0, 'units': 8, 'month': '2026-09'},
    {'region': 'APAC', 'product': 'widget', 'amount': 140.25, 'units': 1, 'month': '2026-09'},
    {'region': 'LATAM', 'product': 'widget', 'amount': 990.0, 'units': 9, 'month': '2026-09'},
    {'region': 'LATAM', 'product': 'gadget', 'amount': 65.0, 'units': 1, 'month': '2026-09'},
]


def _frame() -> FrameProxy:
    return FrameProxy(Frame([dict(r) for r in _SALES]), allowed_methods='all', lazy_attrs={'height'})


STUBS = '''
from typing import Any

class GroupBy:
    def agg(self, sum_of: str | None = None, count: bool = False) -> Frame:
        """One row per group, with `<sum_of>_sum` and/or `count` columns."""
        ...

class Frame:
    height: int
    """Number of rows."""

    def filter(self, col: str, op: str, value: Any) -> Frame:
        """Rows where `col <op> value`; `op` is `==`, `!=`, `<`, `<=`, `>` or `>=`."""
        ...
    def select(self, cols: list[str]) -> Frame: ...
    def sort(self, col: str, descending: bool = False) -> Frame: ...
    def head(self, n: int) -> Frame: ...
    def group_by(self, col: str) -> GroupBy: ...
    def to_dicts(self) -> list[dict[str, Any]]: ...

df: Frame
"""Sales line items with columns `region`, `product`, `amount`, `units`, `month`."""
'''

_BIG = Frame(list(_SALES)).filter('units', '>=', 5)
EXPECTED = {
    'rows_considered': _BIG.height,
    'top': _BIG.group_by('region')
    .agg(sum_of='amount', count=True)
    .sort('amount_sum', descending=True)
    .head(2)
    .to_dicts(),
}

REFERENCE = """
big = df.filter('units', '>=', 5)
top = big.group_by('region').agg(sum_of='amount', count=True).sort('amount_sum', descending=True).head(2)
{'rows_considered': big.height, 'top': top.to_dicts()}
"""

TASK = Task(
    name='polars_proxy',
    category='proxy',
    prompt=(
        'Using the DataFrame `df`, consider only line items with at least 5 units. Total the amount '
        'and count the items per region, and return the two regions with the highest total as a list '
        'of dicts with "region", "amount_sum" and "count", highest first, under the key "top". Also '
        'return "rows_considered", the number of line items with at least 5 units.'
    ),
    stubs=STUBS,
    tools={},
    inputs={'df': _frame()},
    expected=EXPECTED,
    evaluators=(EqualsExpected(),),
    reference_solution=REFERENCE,
    traps=('method chaining through host proxies', 'lazy attribute reads', 'sync host methods'),
    max_result_bytes=300,
)
