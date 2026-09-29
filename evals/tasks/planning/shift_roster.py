"""Build a week's rota under availability, role and hour-cap constraints.

There is no single right answer, so a predicate checks feasibility instead. The
natural search enumerates `itertools.combinations` of eligible staff per shift; Monty
does not implement `combinations`, so the reference records that gap until it lands.
"""

from __future__ import annotations

import asyncio
import itertools
from typing import Any, cast

from evals.harness.evaluators import Predicate
from evals.harness.task import Task

SHIFT_HOURS = 8
HOST_LATENCY = 0.005
"""Simulated round trip per host call, so gathered fetches overlap and `call_batches` can see them."""
MAX_HOURS = 40
_DAYS = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun']

_STAFF: list[dict[str, Any]] = [
    {'id': 'ana', 'role': 'senior', 'available': ['mon', 'tue', 'wed', 'thu', 'fri']},
    {'id': 'ben', 'role': 'senior', 'available': ['wed', 'thu', 'fri', 'sat', 'sun']},
    {'id': 'cai', 'role': 'senior', 'available': ['mon', 'tue', 'sat', 'sun']},
    {'id': 'dee', 'role': 'junior', 'available': ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun']},
    {'id': 'eli', 'role': 'junior', 'available': ['mon', 'wed', 'fri', 'sun']},
    {'id': 'fay', 'role': 'junior', 'available': ['tue', 'thu', 'sat', 'sun']},
    {'id': 'gus', 'role': 'junior', 'available': ['mon', 'tue', 'wed', 'thu', 'fri']},
]

_SHIFTS: list[dict[str, Any]] = [
    {'id': f'{day}-{slot}', 'day': day, 'headcount': 2 if slot == 'am' else 3 if day in ('fri', 'sat') else 2}
    for day in _DAYS
    for slot in ('am', 'pm')
]


async def fetch_staff() -> list[dict[str, Any]]:
    """Host function: staff, their role and the days they can work."""
    await asyncio.sleep(HOST_LATENCY)
    return [dict(person) for person in _STAFF]


async def fetch_shifts() -> list[dict[str, Any]]:
    """Host function: the week's shifts and required headcount."""
    await asyncio.sleep(HOST_LATENCY)
    return [dict(shift) for shift in _SHIFTS]


STUBS = '''
from typing import Any, cast

SHIFT_HOURS: int = 8
MAX_HOURS: int = 40

async def fetch_staff() -> list[dict[str, Any]]:
    """Return staff: `id`, `role` (`"senior"` or `"junior"`), `available` (day names)."""
    ...

async def fetch_shifts() -> list[dict[str, Any]]:
    """Return the week's shifts in order: `id` like `"mon-am"`, `day`, `headcount`."""
    ...
'''


def _feasible(result: object) -> bool:
    """Every shift fully staffed by available people with a senior, nobody over hours or doubled up."""
    if not isinstance(result, dict):
        return False
    roster = cast('dict[str, object]', result)
    staff = {person['id']: person for person in _STAFF}
    hours: dict[str, int] = {}
    for shift in _SHIFTS:
        assigned = roster.get(shift['id'])
        if not isinstance(assigned, list) or len(cast('list[object]', assigned)) != shift['headcount']:
            return False
        names = [str(name) for name in cast('list[object]', assigned)]
        if len(set(names)) != len(names) or any(name not in staff for name in names):
            return False
        if any(shift['day'] not in staff[name]['available'] for name in names):
            return False
        if not any(staff[name]['role'] == 'senior' for name in names):
            return False
        for name in names:
            hours[name] = hours.get(name, 0) + SHIFT_HOURS
    return all(total <= MAX_HOURS for total in hours.values()) and len(roster) == len(_SHIFTS)


REFERENCE = """
import asyncio
import itertools

staff, shifts = await asyncio.gather(fetch_staff(), fetch_shifts())
by_id = {person['id']: person for person in staff}

def solve(index, hours, roster):
    if index == len(shifts):
        return roster
    shift = shifts[index]
    eligible = [
        p['id'] for p in staff
        if shift['day'] in p['available'] and hours.get(p['id'], 0) + SHIFT_HOURS <= MAX_HOURS
    ]
    for team in itertools.combinations(eligible, shift['headcount']):
        if not any(by_id[name]['role'] == 'senior' for name in team):
            continue
        next_hours = dict(hours)
        for name in team:
            next_hours[name] = next_hours.get(name, 0) + SHIFT_HOURS
        roster[shift['id']] = list(team)
        found = solve(index + 1, next_hours, roster)
        if found is not None:
            return found
    return None

solve(0, {}, {})
"""


def _check_solvable() -> None:
    """The fixture must have a solution reachable by the reference's search."""
    by_id = {person['id']: person for person in _STAFF}

    def solve(index: int, hours: dict[str, int], roster: dict[str, list[str]]) -> dict[str, list[str]] | None:
        if index == len(_SHIFTS):
            return roster
        shift = _SHIFTS[index]
        eligible: list[str] = [
            p['id'] for p in _STAFF if shift['day'] in p['available'] and hours.get(p['id'], 0) + 8 <= 40
        ]
        headcount: int = shift['headcount']
        for team in itertools.combinations(eligible, headcount):
            if not any(by_id[name]['role'] == 'senior' for name in team):
                continue
            next_hours = dict(hours)
            for name in team:
                next_hours[name] = next_hours.get(name, 0) + 8
            roster[shift['id']] = list(team)
            found = solve(index + 1, next_hours, roster)
            if found is not None:
                return found
        return None

    solution = solve(0, {}, {})
    assert solution is not None and _feasible(solution), 'shift_roster fixture has no feasible roster'


_check_solvable()

TASK = Task(
    name='shift_roster',
    category='planning',
    prompt=(
        'Build a rota for the week. Every shift must have exactly its headcount of distinct '
        'staff, each available on that day, with at least one senior. A shift is SHIFT_HOURS '
        'long and nobody may exceed MAX_HOURS in the week. Return a dict mapping each shift id '
        'to the list of staff ids on it.'
    ),
    stubs=STUBS,
    tools={'fetch_staff': fetch_staff, 'fetch_shifts': fetch_shifts},
    inputs={'SHIFT_HOURS': SHIFT_HOURS, 'MAX_HOURS': MAX_HOURS},
    evaluators=(Predicate('every shift fully and legally staffed', _feasible),),
    reference_solution=REFERENCE,
    traps=('itertools.combinations', 'recursion for backtracking', 'ortools reflex'),
    expected_external_calls=2,
    expected_call_batches=1,
)
