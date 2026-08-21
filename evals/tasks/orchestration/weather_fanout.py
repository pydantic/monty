"""Fan out one host call per city, convert, and rank — the canonical code-mode shape.

This is the task that measures the time axis. Twelve cities means twelve host calls
whichever way the code is written, but awaiting them in a loop costs twelve round trips
where `asyncio.gather` costs one. Both spellings return the same answer, so correctness
alone cannot tell them apart — `expected_call_batches` is what separates them.
"""

from __future__ import annotations

import asyncio
from typing import Any

from evals.harness.task import Approx, Task

HOST_LATENCY = 0.02
"""Simulated round-trip latency per host call, and load-bearing for the metric.

A host function that returns without ever awaiting completes inside its own coroutine
step, so gathered calls never overlap in wall-clock time and the round-trip counter
reads 12 batches even for a correct `asyncio.gather`. Verified both ways against the
built worker: with latency, gathered calls record 1 batch and sequential ones 12.

Any task that scores `expected_call_batches` needs this; without it the metric silently
reports every solution as fully sequential.
"""

_TEMPERATURES_F = {
    'Reykjavik': 41,
    'Helsinki': 45,
    'Oslo': 48,
    'Moscow': 50,
    'Toronto': 55,
    'Berlin': 61,
    'London': 63,
    'Paris': 66,
    'Tokyo': 72,
    'Athens': 84,
    'Cairo': 95,
    'Dubai': 104,
}

CITIES = list(_TEMPERATURES_F)


async def get_weather(city: str) -> dict[str, Any]:
    """Host function: current conditions for one city, with realistic latency."""
    if city not in _TEMPERATURES_F:
        raise ValueError(f'unknown city: {city}')
    await asyncio.sleep(HOST_LATENCY)
    return {'city': city, 'temp_f': _TEMPERATURES_F[city], 'condition': 'clear'}


STUBS = '''
from typing import Any

CITIES: list[str] = []
"""The twelve cities to report on."""

async def get_weather(city: str) -> dict[str, Any]:
    """Get current weather for a city.

    Returns a dict with keys `city`, `temp_f` (Fahrenheit, int) and `condition`.
    """
    ...
'''

# (41F, 45F, 48F) are the three coldest; C = (F - 32) * 5/9 rounded to one decimal.
EXPECTED = [
    {'city': 'Reykjavik', 'temp_c': 5.0},
    {'city': 'Helsinki', 'temp_c': 7.2},
    {'city': 'Oslo', 'temp_c': 8.9},
]

REFERENCE = """
import asyncio

reports = await asyncio.gather(*[get_weather(city) for city in CITIES])
converted = [
    {'city': r['city'], 'temp_c': round((r['temp_f'] - 32) * 5 / 9, 1)}
    for r in reports
]
sorted(converted, key=lambda r: r['temp_c'])[:3]
"""

TASK = Task(
    name='weather_fanout',
    category='orchestration',
    prompt=(
        'For every city in CITIES, get the current temperature and convert it to Celsius. '
        'Return the three coldest as a list of dicts with keys "city" and "temp_c", '
        'coldest first, with temp_c rounded to one decimal place.'
    ),
    stubs=STUBS,
    tools={'get_weather': get_weather},
    inputs={'CITIES': CITIES},
    expected=Approx(EXPECTED),
    reference_solution=REFERENCE,
    traps=('asyncio.gather', 'sorted key/reverse are keyword-only'),
    expected_external_calls=len(CITIES),
    expected_call_batches=1,
)
