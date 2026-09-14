"""Book an 8 am local appointment on every weekday of next month, in three regions.

The PyCon calendar agent's task, extended across regions on a month with two DST
changes (London on 25 October 2026, Sydney on 4 October). Monty has no `zoneinfo`,
so the host supplies each region's UTC offset per date and the code builds aware
`datetime`s with fixed-offset `timezone`s. Those datetimes cross the host boundary as
arguments, and existing appointments come back as ISO-8601 strings with offsets.
"""

from __future__ import annotations

from datetime import date, datetime, timedelta, timezone
from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.evaluators import Predicate
from evals.harness.task import Task

TODAY = '2026-09-15'
TITLE = 'Daily stand-up'
DURATION = 30

_DST_CHANGE = {
    # region: (offset before, change date, offset from that date)
    'London': (1, date(2026, 10, 25), 0),
    'New York': (-4, date(2026, 11, 1), -5),
    'Sydney': (10, date(2026, 10, 4), 11),
}
REGIONS = list(_DST_CHANGE)


def _offset(region: str, day: date) -> int:
    before, change, after = _DST_CHANGE[region]
    return after if day >= change else before


def _local(region: str, day: date, hour: int) -> datetime:
    return datetime(day.year, day.month, day.day, hour, tzinfo=timezone(timedelta(hours=_offset(region, day))))


_SEED = [
    ('London', date(2026, 10, 6), 8),
    ('New York', date(2026, 10, 14), 8),
    ('Sydney', date(2026, 10, 5), 8),
    ('New York', date(2026, 10, 20), 9),
]

_calendars: dict[str, list[dict[str, Any]]] = {}
_created: list[tuple[str, datetime]] = []


def _reset() -> None:
    """Restore the seeded appointments and forget what the last attempt created."""
    _calendars.clear()
    _created.clear()
    for region in REGIONS:
        _calendars[region] = []
    for region, day, hour in _SEED:
        _calendars[region].append(
            {
                'id': f'seed-{len(_created)}',
                'start': _local(region, day, hour),
                'duration_minutes': 60,
                'title': 'Existing',
            }
        )


async def utc_offset_hours(region: str, day: str) -> int:
    """Host function: the region's UTC offset in hours on an ISO date."""
    return _offset(region, date.fromisoformat(day))


async def get_appointments(region: str, start: str, end: str) -> list[dict[str, Any]]:
    """Host function: appointments starting in `[start, end)`, as ISO-8601 strings with offsets."""
    lo, hi = datetime.fromisoformat(start), datetime.fromisoformat(end)
    return [{**a, 'start': a['start'].isoformat()} for a in _calendars[region] if lo <= a['start'] < hi]


async def create_appointment(region: str, start: datetime, duration_minutes: int, title: str) -> str:
    """Host function: create an appointment; `start` must be timezone-aware."""
    if start.tzinfo is None:
        raise ValueError('start must be timezone-aware')
    apt_id = f'{region.lower().replace(" ", "-")}-{len(_calendars[region]) + 1}'
    _calendars[region].append({'id': apt_id, 'start': start, 'duration_minutes': duration_minutes, 'title': title})
    _created.append((region, start.astimezone(timezone.utc)))
    return apt_id


STUBS = '''
from datetime import datetime
from typing import Any

TODAY: str = ''
"""Today's date, ISO-8601."""
REGIONS: list[str] = []
"""The regions to book in."""
TITLE: str = ''
"""Title for the new appointments."""
DURATION: int = 0
"""Duration for the new appointments, minutes."""

async def utc_offset_hours(region: str, day: str) -> int:
    """UTC offset of `region` on the ISO date `day`, in whole hours (DST included)."""
    ...

async def get_appointments(region: str, start: str, end: str) -> list[dict[str, Any]]:
    """Appointments in `region` starting at or after `start` and before `end` (ISO-8601 with offset).

    Each has `id`, `start` (ISO-8601 string with offset), `duration_minutes` and `title`.
    """
    ...

async def create_appointment(region: str, start: datetime, duration_minutes: int, title: str) -> str:
    """Create an appointment starting at the timezone-aware `start`; returns its id."""
    ...
'''


def _expected_instants() -> set[tuple[str, datetime]]:
    out: set[tuple[str, datetime]] = set()
    seeded = {(r, d) for r, d, h in _SEED if h == 8}
    for day in (date(2026, 10, 1) + timedelta(days=i) for i in range(31)):
        if day.weekday() >= 5:
            continue
        for region in REGIONS:
            if (region, day) not in seeded:
                out.add((region, _local(region, day, 8).astimezone(timezone.utc)))
    return out


EXPECTED_INSTANTS = _expected_instants()


def _created_as_expected(_result: object) -> bool:
    """Exactly the expected UTC instants were booked, each once, with the right title and length."""
    if len(_created) != len(EXPECTED_INSTANTS) or set(_created) != EXPECTED_INSTANTS:
        return False
    new = [a for region in REGIONS for a in _calendars[region] if not a['id'].startswith('seed-')]
    return all(a['title'] == TITLE and a['duration_minutes'] == DURATION for a in new)


REFERENCE = """
from datetime import date, datetime, timedelta, timezone

today = date.fromisoformat(TODAY)
first = date(today.year + (1 if today.month == 12 else 0), 1 if today.month == 12 else today.month + 1, 1)
after = date(first.year + (1 if first.month == 12 else 0), 1 if first.month == 12 else first.month + 1, 1)

created = 0
for region in REGIONS:
    existing = await get_appointments(region, first.isoformat() + 'T00:00:00+14:00', after.isoformat() + 'T00:00:00-12:00')
    taken = [datetime.fromisoformat(a['start']) for a in existing]
    day = first
    while day < after:
        if day.weekday() < 5:
            offset = await utc_offset_hours(region, day.isoformat())
            tz = timezone(timedelta(hours=offset))
            start = datetime(day.year, day.month, day.day, 8, tzinfo=tz)
            if not any(t == start for t in taken):
                await create_appointment(region, start, DURATION, TITLE)
                created += 1
        day = day + timedelta(days=1)

{'created': created}
"""

TASK = Task(
    name='calendar_fill',
    category='dates',
    prompt=(
        f'For each region in REGIONS, create a "{TITLE}" appointment of {DURATION} minutes at 08:00 local '
        'time on every weekday (Monday to Friday) of next month, relative to TODAY. Skip any day and '
        'region that already has an appointment starting at exactly that instant. Local time means the '
        "region's own clock, including daylight-saving changes during the month; use utc_offset_hours "
        'for the offset on each date. Return {"created": <number of appointments you created>}.'
    ),
    stubs=STUBS,
    tools={
        'utc_offset_hours': utc_offset_hours,
        'get_appointments': get_appointments,
        'create_appointment': create_appointment,
    },
    inputs={'TODAY': TODAY, 'REGIONS': REGIONS, 'TITLE': TITLE, 'DURATION': DURATION},
    expected={'created': len(EXPECTED_INSTANTS)},
    evaluators=(EqualsExpected(), Predicate('booked exactly the expected UTC instants', _created_as_expected)),
    reference_solution=REFERENCE,
    traps=('zoneinfo', 'DST change mid-month', 'aware datetime across the host boundary', 'calendar.monthrange'),
    setup=_reset,
)
