"""Find overlapping meetings and total the busy time.

Monty has `datetime`, `date`, `timedelta`, `timezone` and `strptime`, but no
`datetime.time` class — so the natural "parse the clock time, combine with the date"
route is closed and the model has to work in `datetime` throughout. Interval overlap is
also a place models reliably write `a.start < b.end and b.start < a.end` backwards, so a
wrong answer here is informative even when nothing was missing.
"""

from __future__ import annotations

from datetime import datetime

from evals.harness.task import Approx, Task

_MEETINGS = [
    {'title': 'standup', 'start': '2026-08-21T09:00:00', 'end': '2026-08-21T09:15:00'},
    {'title': 'design review', 'start': '2026-08-21T09:00:00', 'end': '2026-08-21T10:00:00'},
    {'title': '1:1', 'start': '2026-08-21T10:00:00', 'end': '2026-08-21T10:30:00'},
    {'title': 'roadmap', 'start': '2026-08-21T10:15:00', 'end': '2026-08-21T11:30:00'},
    {'title': 'lunch', 'start': '2026-08-21T12:00:00', 'end': '2026-08-21T13:00:00'},
    {'title': 'incident review', 'start': '2026-08-21T12:30:00', 'end': '2026-08-21T14:00:00'},
    {'title': 'retro', 'start': '2026-08-21T15:00:00', 'end': '2026-08-21T16:00:00'},
]


async def fetch_meetings() -> list[dict[str, str]]:
    """Host function: today's calendar entries."""
    return [dict(meeting) for meeting in _MEETINGS]


STUBS = '''
async def fetch_meetings() -> list[dict[str, str]]:
    """Return today's meetings.

    Each has `title`, `start` and `end` as ISO-8601 strings like
    `2026-08-21T09:00:00`. Meetings are not necessarily sorted.
    """
    ...
'''


def _expected() -> dict[str, object]:
    parsed = [
        (
            str(meeting['title']),
            datetime.fromisoformat(str(meeting['start'])),
            datetime.fromisoformat(str(meeting['end'])),
        )
        for meeting in _MEETINGS
    ]
    conflicts: list[list[str]] = []
    for i in range(len(parsed)):
        for j in range(i + 1, len(parsed)):
            _, start_a, end_a = parsed[i]
            _, start_b, end_b = parsed[j]
            if start_a < end_b and start_b < end_a:
                conflicts.append(sorted([parsed[i][0], parsed[j][0]]))
    # Union of intervals, so double-booked time is counted once.
    intervals = sorted((start, end) for _, start, end in parsed)
    merged: list[list[datetime]] = []
    for start, end in intervals:
        if merged and start <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], end)
        else:
            merged.append([start, end])
    busy = sum((end - start).total_seconds() for start, end in merged) / 3600
    return {'conflicts': sorted(conflicts), 'busy_hours': round(busy, 2)}


# Tuples rather than dicts, deliberately. A dict holding both a title and two
# datetimes types as `dict[str, str | datetime]`, and subtracting two of its values is
# then rejected by the bundled type checker before the code ever runs. A tuple keeps
# each field's type positionally, so the arithmetic checks. See `evals/README.md`.
REFERENCE = """
from datetime import datetime

meetings = await fetch_meetings()
parsed = []
for meeting in meetings:
    parsed.append((
        meeting['title'],
        datetime.fromisoformat(meeting['start']),
        datetime.fromisoformat(meeting['end']),
    ))

conflicts = []
for i in range(len(parsed)):
    for j in range(i + 1, len(parsed)):
        if parsed[i][1] < parsed[j][2] and parsed[j][1] < parsed[i][2]:
            conflicts.append(sorted([parsed[i][0], parsed[j][0]]))

ordered = sorted(parsed, key=lambda meeting: meeting[1])
merged = []
for meeting in ordered:
    if merged and meeting[1] <= merged[-1][1]:
        if meeting[2] > merged[-1][1]:
            merged[-1][1] = meeting[2]
    else:
        merged.append([meeting[1], meeting[2]])

seconds = 0.0
for block in merged:
    seconds = seconds + (block[1] - block[0]).total_seconds()

{'conflicts': sorted(conflicts), 'busy_hours': round(seconds / 3600, 2)}
"""

TASK = Task(
    name='schedule_conflicts',
    category='dates',
    prompt=(
        'Find every pair of meetings that overlap in time. Return a dict with '
        '"conflicts" (a sorted list of [title_a, title_b] pairs, each pair itself sorted '
        'alphabetically) and "busy_hours" (the total time covered by at least one '
        'meeting, counting overlapping time only once, rounded to 2 decimal places).'
    ),
    stubs=STUBS,
    tools={'fetch_meetings': fetch_meetings},
    expected=Approx(_expected()),
    reference_solution=REFERENCE,
    traps=('datetime.time', 'no timedelta division', 'interval overlap logic'),
    expected_external_calls=1,
)
