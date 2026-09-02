"""Retry a flaky host call and report what stayed broken.

The natural CPython solution reaches for two things Monty does not have: a custom
exception class to distinguish "retryable" from "fatal", and `time.sleep` for a backoff.
Both are avoidable — builtin exceptions carry a message, and a sandbox has nothing to
back off *for* — but a model has to be told, which is what makes this a prompt test.

The tool is stateful on purpose: record 5 fails twice and then succeeds, so a solution
that gives up after one attempt gets a different answer from one that retries three
times. Retrying is therefore observable in the result, not just in the call count.
"""

from __future__ import annotations

from typing import Any

from evals.harness.task import Exact, Task

RECORD_IDS = [1, 2, 3, 4, 5, 6, 7, 8]
_ALWAYS_FAIL = {3, 7}
_FAILS_TWICE = {5: 2}

_attempts: dict[int, int] = {}


def reset_state() -> None:
    """Clear the per-record attempt counters between task attempts."""
    _attempts.clear()


async def fetch_record(record_id: int) -> dict[str, Any]:
    """Host function: fetch a record, unreliably.

    Records 3 and 7 are permanently broken. Record 5 fails its first two attempts.
    """
    seen = _attempts.get(record_id, 0) + 1
    _attempts[record_id] = seen
    if record_id in _ALWAYS_FAIL:
        raise ValueError(f'record {record_id} is corrupt')
    if seen <= _FAILS_TWICE.get(record_id, 0):
        raise ValueError(f'record {record_id} temporarily unavailable')
    return {'id': record_id, 'name': f'record-{record_id}'}


STUBS = '''
from typing import Any

RECORD_IDS: list[int] = []
"""The record ids to fetch."""

async def fetch_record(record_id: int) -> dict[str, Any]:
    """Fetch one record by id.

    Raises `ValueError` when the record cannot be read. Some failures are transient
    and succeed on a later attempt; others are permanent.
    """
    ...
'''

REFERENCE = """
failed = []
records = []
for record_id in RECORD_IDS:
    for attempt in range(3):
        try:
            records.append(await fetch_record(record_id))
            break
        except ValueError:
            if attempt == 2:
                failed.append(record_id)

{'fetched': len(records), 'failed': sorted(failed)}
"""

TASK = Task(
    name='retry_flaky',
    category='orchestration',
    prompt=(
        'Fetch every record in RECORD_IDS. Some fail transiently, so retry a failed '
        'record up to 3 times in total before giving up on it. Return a dict with '
        '"fetched" (how many records you successfully read) and "failed" (the sorted '
        'list of ids that failed all 3 attempts).'
    ),
    stubs=STUBS,
    tools={'fetch_record': fetch_record},
    inputs={'RECORD_IDS': RECORD_IDS},
    expected=Exact({'fetched': 6, 'failed': [3, 7]}),
    reference_solution=REFERENCE,
    traps=('custom exception classes', 'time.sleep', 'functools.wraps'),
    # 6 clean records + 2 permanent failures × 3 attempts + record 5's 2 retries.
    expected_external_calls=6 + len(_ALWAYS_FAIL) * 3 + 2,
    setup=reset_state,
)
