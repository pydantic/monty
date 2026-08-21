"""Two-level group-by with sums — the single most common shape in agent data work.

`itertools.groupby` is the reflex here and Monty does not have it. It is also the wrong
reflex in CPython: `groupby` only groups *adjacent* equal keys, so it needs a sort
first, and a model that reaches for it without sorting gets a silently wrong answer
even where it exists. Building a dict of lists is both available and correct.
"""

from __future__ import annotations

from typing import Any

from evals.harness.task import Approx, Task

_SALES = [
    {'region': 'EMEA', 'product': 'widget', 'amount': 1200.0, 'units': 12},
    {'region': 'AMER', 'product': 'gadget', 'amount': 900.5, 'units': 3},
    {'region': 'EMEA', 'product': 'gadget', 'amount': 450.25, 'units': 5},
    {'region': 'APAC', 'product': 'widget', 'amount': 2100.0, 'units': 21},
    {'region': 'EMEA', 'product': 'widget', 'amount': 300.75, 'units': 3},
    {'region': 'AMER', 'product': 'widget', 'amount': 1500.0, 'units': 15},
    {'region': 'APAC', 'product': 'gadget', 'amount': 725.5, 'units': 7},
    {'region': 'AMER', 'product': 'gadget', 'amount': 610.0, 'units': 2},
    {'region': 'EMEA', 'product': 'widget', 'amount': 880.0, 'units': 8},
    {'region': 'APAC', 'product': 'widget', 'amount': 140.25, 'units': 1},
]


async def fetch_sales() -> list[dict[str, Any]]:
    """Host function: every sales line item."""
    return [dict(row) for row in _SALES]


STUBS = '''
from typing import Any

async def fetch_sales() -> list[dict[str, Any]]:
    """Return every sales line item.

    Each row has `region`, `product`, `amount` (float) and `units` (int).
    """
    ...
'''


def _expected() -> dict[str, dict[str, dict[str, Any]]]:
    grouped: dict[str, dict[str, dict[str, Any]]] = {}
    for row in _SALES:
        product = grouped.setdefault(str(row['region']), {}).setdefault(
            str(row['product']), {'amount': 0.0, 'units': 0, 'orders': 0}
        )
        product['amount'] = round(product['amount'] + float(row['amount']), 2)
        product['units'] += int(row['units'])
        product['orders'] += 1
    return grouped


REFERENCE = """
rows = await fetch_sales()

grouped = {}
for row in rows:
    by_product = grouped.setdefault(row['region'], {})
    totals = by_product.setdefault(row['product'], {'amount': 0.0, 'units': 0, 'orders': 0})
    totals['amount'] = round(totals['amount'] + row['amount'], 2)
    totals['units'] = totals['units'] + row['units']
    totals['orders'] = totals['orders'] + 1

grouped
"""

TASK = Task(
    name='group_by_report',
    category='wrangling',
    prompt=(
        'Group the sales rows by region and then by product. Return a nested dict '
        '{region: {product: {"amount": total amount rounded to 2dp, "units": total '
        'units, "orders": number of rows}}}.'
    ),
    stubs=STUBS,
    tools={'fetch_sales': fetch_sales},
    expected=Approx(_expected()),
    reference_solution=REFERENCE,
    traps=('itertools.groupby', 'collections.OrderedDict', 'collections.defaultdict nesting'),
    expected_external_calls=1,
)
