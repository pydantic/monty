"""Reconcile the same people across a CRM, a ticketing system and a calendar.

Nine host functions, three per system, over in-memory stores that disagree on
which people exist and how their emails are spelled. The code has to end with every
person present exactly once in every system under a canonical lower-case email, so
the scoring is a check on the final state, not on the answer: creating a duplicate
or leaving an unnormalised email fails it whatever the returned counts say.
"""

from __future__ import annotations

from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.evaluators import Predicate
from evals.harness.task import Task

_PEOPLE = {
    'ada@example.com': 'Ada Lovelace',
    'grace@example.com': 'Grace Hopper',
    'alan@example.com': 'Alan Turing',
    'edsger@example.com': 'Edsger Dijkstra',
    'barbara@example.com': 'Barbara Liskov',
    'donald@example.com': 'Donald Knuth',
    'margaret@example.com': 'Margaret Hamilton',
    'tony@example.com': 'Tony Hoare',
}

_SEED: dict[str, list[tuple[str, str]]] = {
    'crm': [
        ('Ada Lovelace', ' Ada@Example.com'),
        ('Grace Hopper', 'grace@example.com'),
        ('Alan Turing', 'ALAN@example.com '),
        ('Edsger Dijkstra', 'edsger@example.com'),
        ('Barbara Liskov', 'barbara@example.com'),
        ('Donald Knuth', 'Donald@Example.COM'),
    ],
    'tickets': [
        ('Ada Lovelace', 'ada@example.com'),
        ('Grace Hopper', 'Grace@example.com'),
        ('Barbara Liskov', 'barbara@example.com '),
        ('Margaret Hamilton', 'margaret@example.com'),
        ('Tony Hoare', 'TONY@EXAMPLE.COM'),
    ],
    'calendar': [
        ('Alan Turing', 'alan@example.com'),
        ('Edsger Dijkstra', ' edsger@example.com'),
        ('Margaret Hamilton', 'Margaret@example.com'),
        ('Tony Hoare', 'tony@example.com'),
        ('Donald Knuth', 'donald@example.com'),
    ],
}

_stores: dict[str, list[dict[str, Any]]] = {}


def _reset() -> None:
    for system, rows in _SEED.items():
        _stores[system] = [{'id': f'{system}-{i + 1}', 'name': n, 'email': e} for i, (n, e) in enumerate(rows)]


def _list(system: str) -> list[dict[str, Any]]:
    return [dict(r) for r in _stores[system]]


def _create(system: str, name: str, email: str) -> str:
    row = {'id': f'{system}-{len(_stores[system]) + 1}', 'name': name, 'email': email}
    _stores[system].append(row)
    return row['id']


def _update(system: str, record_id: str, email: str) -> bool:
    for row in _stores[system]:
        if row['id'] == record_id:
            row['email'] = email
            return True
    raise KeyError(f'no {system} record {record_id}')


async def crm_list() -> list[dict[str, Any]]:
    return _list('crm')


async def crm_create(name: str, email: str) -> str:
    return _create('crm', name, email)


async def crm_update(record_id: str, email: str) -> bool:
    return _update('crm', record_id, email)


async def tickets_list() -> list[dict[str, Any]]:
    return _list('tickets')


async def tickets_create(name: str, email: str) -> str:
    return _create('tickets', name, email)


async def tickets_update(record_id: str, email: str) -> bool:
    return _update('tickets', record_id, email)


async def calendar_list() -> list[dict[str, Any]]:
    return _list('calendar')


async def calendar_create(name: str, email: str) -> str:
    return _create('calendar', name, email)


async def calendar_update(record_id: str, email: str) -> bool:
    return _update('calendar', record_id, email)


STUBS = '''
from typing import Any

async def crm_list() -> list[dict[str, Any]]:
    """Every CRM contact: `id`, `name`, `email`."""
    ...
async def crm_create(name: str, email: str) -> str:
    """Create a CRM contact; returns its id."""
    ...
async def crm_update(record_id: str, email: str) -> bool:
    """Change a CRM contact's email."""
    ...
async def tickets_list() -> list[dict[str, Any]]:
    """Every ticketing user: `id`, `name`, `email`."""
    ...
async def tickets_create(name: str, email: str) -> str: ...
async def tickets_update(record_id: str, email: str) -> bool: ...
async def calendar_list() -> list[dict[str, Any]]:
    """Every calendar attendee: `id`, `name`, `email`."""
    ...
async def calendar_create(name: str, email: str) -> str: ...
async def calendar_update(record_id: str, email: str) -> bool: ...
'''


def _in_sync(_result: object) -> bool:
    """Each system holds every person exactly once, under the canonical email and the right name."""
    for rows in _stores.values():
        emails = [r['email'] for r in rows]
        if sorted(emails) != sorted(_PEOPLE):
            return False
        if any(_PEOPLE[r['email']] != r['name'] for r in rows):
            return False
    return True


def _expected_counts() -> dict[str, int]:
    created = updated = 0
    for rows in _SEED.values():
        present = {e.strip().lower() for _, e in rows}
        created += len(set(_PEOPLE) - present)
        updated += sum(1 for _, e in rows if e != e.strip().lower())
    return {'created': created, 'updated': updated}


REFERENCE = """
import asyncio

systems = {
    'crm': (crm_list, crm_create, crm_update),
    'tickets': (tickets_list, tickets_create, tickets_update),
    'calendar': (calendar_list, calendar_create, calendar_update),
}

listed = await asyncio.gather(crm_list(), tickets_list(), calendar_list())
records = dict(zip(list(systems), listed))

def canonical(email):
    return email.strip().lower()

people = {}
for rows in records.values():
    for row in rows:
        key = canonical(row['email'])
        if key not in people:
            people[key] = row['name']

created = 0
updated = 0
for name, (_, create, update) in systems.items():
    rows = records[name]
    present = {}
    for row in rows:
        key = canonical(row['email'])
        if key in present:
            continue
        present[key] = row
        if row['email'] != key:
            await update(row['id'], key)
            updated += 1
    for email in people:
        if email not in present:
            await create(people[email], email)
            created += 1

{'created': created, 'updated': updated}
"""

TASK = Task(
    name='crm_sync',
    category='business',
    prompt=(
        'The CRM, the ticketing system and the calendar should all hold the same people. Treat two '
        'records as the same person when their emails match after trimming whitespace and ignoring '
        'case. Make every person exist exactly once in every system, with their email stored in '
        'canonical form (trimmed, lower-case): update records whose email is not canonical, and create '
        'the people a system is missing, using the name known elsewhere. Never create a duplicate. '
        'Return {"created": <records created>, "updated": <records whose email you changed>}.'
    ),
    stubs=STUBS,
    tools={
        'crm_list': crm_list,
        'crm_create': crm_create,
        'crm_update': crm_update,
        'tickets_list': tickets_list,
        'tickets_create': tickets_create,
        'tickets_update': tickets_update,
        'calendar_list': calendar_list,
        'calendar_create': calendar_create,
        'calendar_update': calendar_update,
    },
    expected=_expected_counts(),
    evaluators=(EqualsExpected(), Predicate('every system holds each person once, canonically', _in_sync)),
    reference_solution=REFERENCE,
    traps=('idempotent writes across three systems', 'email normalisation', 'many distinct host functions'),
    setup=_reset,
)
