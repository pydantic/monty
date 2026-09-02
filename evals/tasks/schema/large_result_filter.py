"""Pull 2,000 records and return 5 — the context-saving claim, measured.

This is the task where code mode should beat JSON tool calling outright: the whole
result set never enters the model's context, only the answer does. `max_result_bytes`
enforces that, so a solution that returns all 2,000 rows fails even though the answer is
technically inside it.

The traps are laziness assumptions. `filter` returns a list in Monty, not an iterator,
so `next(filter(...))` fails; and `sorted`'s `key` is keyword-only.
"""

from __future__ import annotations

from typing import Any

from evals.harness.task import Every, Exact, Predicate, Task

_SEVERITIES = ['info', 'info', 'warning', 'info', 'debug', 'warning', 'info', 'debug']
_CRITICAL_AT = {417, 1203, 88, 1876, 942}


def _build_events() -> list[dict[str, Any]]:
    """2,000 events, of which exactly five are critical."""
    events: list[dict[str, Any]] = []
    for index in range(2000):
        severity = 'critical' if index in _CRITICAL_AT else _SEVERITIES[index % len(_SEVERITIES)]
        events.append(
            {
                'id': f'evt-{index:05d}',
                'severity': severity,
                'timestamp': f'2026-08-21T{index // 100:02d}:{index % 60:02d}:00',
                'service': f'svc-{index % 7}',
                'message': f'event number {index} occurred while handling a request',
            }
        )
    return events


_EVENTS = _build_events()


async def fetch_events() -> list[dict[str, Any]]:
    """Host function: the full event stream."""
    return [dict(event) for event in _EVENTS]


STUBS = '''
from typing import Any

async def fetch_events() -> list[dict[str, Any]]:
    """Return the full event stream (about 2,000 events).

    Each event has `id`, `severity`, `timestamp`, `service` and `message`.
    """
    ...
'''

_EXPECTED = sorted(
    ({'id': event['id'], 'service': event['service']} for event in _EVENTS if event['severity'] == 'critical'),
    key=lambda event: str(event['id']),
)

MAX_RESULT_BYTES = 400

TASK = Task(
    name='large_result_filter',
    category='schema',
    prompt=(
        'Find every event with severity "critical". Return them as a list of dicts with '
        'only the keys "id" and "service", sorted by id ascending. Return nothing else.'
    ),
    stubs=STUBS,
    tools={'fetch_events': fetch_events},
    expected=Every(
        (
            Exact(_EXPECTED),
            Predicate(
                f'result serialises to under {MAX_RESULT_BYTES} bytes',
                lambda result: len(repr(result)) < MAX_RESULT_BYTES,
            ),
        )
    ),
    reference_solution="""
events = await fetch_events()
critical = [
    {'id': event['id'], 'service': event['service']}
    for event in events
    if event['severity'] == 'critical'
]
sorted(critical, key=lambda event: event['id'])
""",
    traps=('filter/map are eager', 'sorted key is keyword-only', 'dataclasses.asdict'),
    expected_external_calls=1,
    max_result_bytes=MAX_RESULT_BYTES,
)
