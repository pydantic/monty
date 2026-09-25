"""A refund run that pauses for a human at each large refund and resumes elsewhere.

`request_approval` is a sync host function; the executor snapshots the run at every
call to it, discards the session, restores the dump in a fresh session and answers
the call there. The code never notices, which is the point: an approval that takes
a day, or a worker that restarts, looks the same from inside.
"""

from __future__ import annotations

from typing import Any

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

THRESHOLD = 500.0
_APPROVE_UP_TO = 900.0

_REFUNDS: list[dict[str, Any]] = [
    {'refund_id': f'R-{index + 1:03d}', 'customer': f'C-{(index * 7) % 13 + 1}', 'amount': amount}
    for index, amount in enumerate(
        [120.0, 640.5, 75.25, 1200.0, 310.0, 505.0, 880.0, 42.0, 950.0, 210.0, 700.0, 15.5, 499.99, 1500.0, 260.0]
    )
]


async def fetch_refunds() -> list[dict[str, Any]]:
    """Host: every refund awaiting processing."""
    return [dict(r) for r in _REFUNDS]


def request_approval(refund_id: str, amount: float) -> bool:
    """Host, sync: ask a human; the run is snapshotted here and resumed later."""
    return amount <= _APPROVE_UP_TO


def _expected() -> dict[str, Any]:
    approved = [r for r in _REFUNDS if r['amount'] <= THRESHOLD or r['amount'] <= _APPROVE_UP_TO]
    declined = [r['refund_id'] for r in _REFUNDS if r['amount'] > _APPROVE_UP_TO]
    return {
        'approved_count': len(approved),
        'approved_total': round(sum(r['amount'] for r in approved), 2),
        'declined': declined,
    }


STUBS = '''
from typing import Any

THRESHOLD: float = 500.0
"""Refunds above this amount need a human decision."""

async def fetch_refunds() -> list[dict[str, Any]]:
    """Return every pending refund: `refund_id`, `customer`, `amount`."""
    ...

def request_approval(refund_id: str, amount: float) -> bool:
    """Ask a human whether to approve one refund. Blocks until they answer; call it without `await`."""
    ...
'''

REFERENCE = """
refunds = await fetch_refunds()

approved_total = 0.0
approved_count = 0
declined = []
for refund in refunds:
    if refund['amount'] > THRESHOLD:
        ok = request_approval(refund['refund_id'], refund['amount'])
    else:
        ok = True
    if ok:
        approved_count = approved_count + 1
        approved_total = approved_total + refund['amount']
    else:
        declined.append(refund['refund_id'])

{'approved_count': approved_count, 'approved_total': round(approved_total, 2), 'declined': declined}
"""

_OVER = sum(1 for r in _REFUNDS if r['amount'] > THRESHOLD)

TASK = Task(
    name='approval_gate',
    category='sandbox',
    prompt=(
        'Process every pending refund. Refunds of THRESHOLD or less are approved automatically; '
        'anything above it must go through `request_approval`, one call per refund, in the '
        'order returned. Return a dict with "approved_count", "approved_total" (rounded to 2 '
        'decimal places) and "declined" (the ids that were refused, in order).'
    ),
    stubs=STUBS,
    tools={'fetch_refunds': fetch_refunds, 'request_approval': request_approval},
    inputs={'THRESHOLD': THRESHOLD},
    expected=_expected(),
    evaluators=(ApproxExpected(),),
    reference_solution=REFERENCE,
    traps=('a sync host function called without await', 'state surviving a snapshot'),
    expected_external_calls=1 + _OVER,
    max_result_bytes=200,
    snapshot_at='request_approval',
)
