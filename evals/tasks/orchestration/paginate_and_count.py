"""Follow a cursor until it runs out, then tally — a loop that cannot be unrolled.

The page count is not known up front, so this cannot be answered by a fixed number of
tool calls decided in advance. It is the shape that most clearly beats JSON tool
calling, and it exercises `collections.Counter`/`defaultdict`, both of which Monty has.
"""

from __future__ import annotations

from typing import Any

from evals.harness.task import Exact, Task

_STATUSES = ['shipped', 'pending', 'cancelled', 'shipped', 'shipped', 'pending', 'refunded', 'shipped']
_PAGE_SIZE = 20
_TOTAL = 94


def _order(index: int) -> dict[str, Any]:
    return {'id': 1000 + index, 'status': _STATUSES[index % len(_STATUSES)], 'total': 10.0 + index}


async def list_orders(cursor: str | None = None) -> dict[str, Any]:
    """Host function: one page of orders plus the cursor for the next."""
    start = 0 if cursor is None else int(cursor)
    if start >= _TOTAL:
        return {'orders': [], 'next_cursor': None}
    page = [_order(i) for i in range(start, min(start + _PAGE_SIZE, _TOTAL))]
    next_start = start + _PAGE_SIZE
    return {'orders': page, 'next_cursor': str(next_start) if next_start < _TOTAL else None}


STUBS = '''
from typing import Any

async def list_orders(cursor: str | None = None) -> dict[str, Any]:
    """List one page of orders.

    Pass `cursor=None` for the first page. Returns a dict with `orders` (a list of
    dicts with `id`, `status` and `total`) and `next_cursor`, which is `None` once
    there are no more pages.
    """
    ...
'''


def _expected_counts() -> dict[str, int]:
    counts: dict[str, int] = {}
    for i in range(_TOTAL):
        status = _STATUSES[i % len(_STATUSES)]
        counts[status] = counts.get(status, 0) + 1
    return counts


REFERENCE = """
counts = {}
cursor = None
while True:
    page = await list_orders(cursor=cursor)
    for order in page['orders']:
        status = order['status']
        counts[status] = counts.get(status, 0) + 1
    cursor = page['next_cursor']
    if cursor is None:
        break

counts
"""

# 94 orders over 20-row pages is five requests, each of which must complete before the
# next cursor is known — so five round trips is the floor, not a failure to parallelise.
_PAGES = -(-_TOTAL // _PAGE_SIZE)

TASK = Task(
    name='paginate_and_count',
    category='orchestration',
    prompt=(
        'Fetch every page of orders and return a dict mapping each order status to the '
        'number of orders with that status.'
    ),
    stubs=STUBS,
    tools={'list_orders': list_orders},
    expected=Exact(_expected_counts()),
    reference_solution=REFERENCE,
    traps=('collections.Counter', 'eager map/filter'),
    expected_external_calls=_PAGES,
    expected_call_batches=_PAGES,
)
