"""Answer a follow-up from session state instead of re-fetching.

Monty sessions keep their globals between `feed_run` calls, so the second question here
is answerable with zero host calls. The existing prompt in `examples/web_scraper`
asserts the opposite — "the python executor is NOT a REPL, you must define all values
each time" — so this task scores a specific, falsifiable claim a prompt variant makes.

The follow-up is graded on its call count as well as its answer: re-fetching everything
still produces the right number, so correctness alone would score both spellings the
same and miss the point.
"""

from __future__ import annotations

from typing import Any

from evals.harness.task import Approx, Task, Turn

_ORDERS = [
    {'id': 'o-1', 'region': 'EMEA', 'amount': 1200.50, 'customer': 'Analytical Engines'},
    {'id': 'o-2', 'region': 'AMER', 'amount': 2300.00, 'customer': 'Compiler Works'},
    {'id': 'o-3', 'region': 'APAC', 'amount': 1875.25, 'customer': 'Shortest Path BV'},
    {'id': 'o-4', 'region': 'EMEA', 'amount': 940.75, 'customer': 'Substitution Inc'},
    {'id': 'o-5', 'region': 'AMER', 'amount': 3105.00, 'customer': 'Bletchley Park'},
    {'id': 'o-6', 'region': 'APAC', 'amount': 615.50, 'customer': 'Turing Systems'},
    {'id': 'o-7', 'region': 'EMEA', 'amount': 2050.00, 'customer': 'Lovelace Ltd'},
]


async def fetch_orders() -> list[dict[str, Any]]:
    """Host function: every order in the current period."""
    return [dict(order) for order in _ORDERS]


STUBS = '''
from typing import Any

async def fetch_orders() -> list[dict[str, Any]]:
    """Return every order in the current period.

    Each order has `id`, `region`, `amount` (float) and `customer`.
    """
    ...
'''

_TOTAL = round(sum(float(order['amount']) for order in _ORDERS), 2)


def _top_region() -> dict[str, Any]:
    by_region: dict[str, float] = {}
    for order in _ORDERS:
        region = str(order['region'])
        by_region[region] = round(by_region.get(region, 0.0) + float(order['amount']), 2)
    best = max(by_region, key=lambda region: by_region[region])
    return {'region': best, 'amount': by_region[best]}


TASK = Task(
    name='followup_reuse',
    category='stateful',
    prompt='Fetch all the orders and return the total revenue, rounded to 2 decimal places.',
    stubs=STUBS,
    tools={'fetch_orders': fetch_orders},
    expected=Approx(_TOTAL),
    reference_solution="""
orders = await fetch_orders()

total = 0.0
for order in orders:
    total = total + order['amount']

round(total, 2)
""",
    traps=('prompt claims about session state',),
    expected_external_calls=1,
    follow_up=Turn(
        prompt=(
            'Which region contributed the most revenue? Return a dict with "region" and '
            '"amount" (rounded to 2 decimal places).'
        ),
        expected=Approx(_top_region()),
        # The orders are already bound in the session; fetching them again is the failure
        # this turn exists to catch.
        expected_external_calls=0,
        reference_solution="""
by_region = {}
for order in orders:
    region = order['region']
    by_region[region] = round(by_region.get(region, 0.0) + order['amount'], 2)

best = sorted(by_region, key=lambda region: by_region[region], reverse=True)[0]

{'region': best, 'amount': by_region[best]}
""",
    ),
)
