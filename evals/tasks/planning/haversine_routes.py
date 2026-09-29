"""Assign deliveries to their nearest depot and order each route greedily.

Great-circle distance needs `radians`, `sin`, `cos` and `atan2`; the route order is
a nearest-neighbour walk from the depot, so the answer is deterministic and the
trigonometry is what the case measures.
"""

from __future__ import annotations

import asyncio
import math
from typing import Any

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

EARTH_RADIUS_KM = 6371.0088
HOST_LATENCY = 0.005
"""Simulated round trip per host call, so gathered fetches overlap and `call_batches` can see them."""

_DEPOTS: list[dict[str, Any]] = [
    {'id': 'LDN', 'lat': 51.5074, 'lng': -0.1278},
    {'id': 'MAN', 'lat': 53.4808, 'lng': -2.2426},
    {'id': 'EDI', 'lat': 55.9533, 'lng': -3.1883},
]

_DELIVERIES: list[dict[str, Any]] = [
    {'id': 'D01', 'lat': 51.4545, 'lng': -2.5879},
    {'id': 'D02', 'lat': 52.4862, 'lng': -1.8904},
    {'id': 'D03', 'lat': 53.8008, 'lng': -1.5491},
    {'id': 'D04', 'lat': 55.8642, 'lng': -4.2518},
    {'id': 'D05', 'lat': 51.7520, 'lng': -1.2577},
    {'id': 'D06', 'lat': 53.4084, 'lng': -2.9916},
    {'id': 'D07', 'lat': 54.9783, 'lng': -1.6178},
    {'id': 'D08', 'lat': 52.2053, 'lng': 0.1218},
    {'id': 'D09', 'lat': 56.4620, 'lng': -2.9707},
    {'id': 'D10', 'lat': 50.8225, 'lng': -0.1372},
    {'id': 'D11', 'lat': 53.9600, 'lng': -1.0873},
    {'id': 'D12', 'lat': 57.1497, 'lng': -2.0943},
    {'id': 'D13', 'lat': 52.9548, 'lng': -1.1581},
    {'id': 'D14', 'lat': 51.4816, 'lng': -3.1791},
    {'id': 'D15', 'lat': 55.9045, 'lng': -3.2986},
]


async def fetch_depots() -> list[dict[str, Any]]:
    """Host function: depot locations."""
    await asyncio.sleep(HOST_LATENCY)
    return [dict(depot) for depot in _DEPOTS]


async def fetch_deliveries() -> list[dict[str, Any]]:
    """Host function: delivery locations."""
    await asyncio.sleep(HOST_LATENCY)
    return [dict(delivery) for delivery in _DELIVERIES]


STUBS = '''
from typing import Any

EARTH_RADIUS_KM: float = 6371.0088

async def fetch_depots() -> list[dict[str, Any]]:
    """Return the depots: `id`, `lat`, `lng` in decimal degrees."""
    ...

async def fetch_deliveries() -> list[dict[str, Any]]:
    """Return the deliveries to make today: `id`, `lat`, `lng` in decimal degrees."""
    ...
'''


def _haversine(a: dict[str, Any], b: dict[str, Any]) -> float:
    lat1, lat2 = math.radians(a['lat']), math.radians(b['lat'])
    dlat = lat2 - lat1
    dlng = math.radians(b['lng'] - a['lng'])
    h = math.sin(dlat / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin(dlng / 2) ** 2
    return 2 * EARTH_RADIUS_KM * math.atan2(math.sqrt(h), math.sqrt(1 - h))


def _expected() -> dict[str, Any]:
    assigned: dict[str, list[dict[str, Any]]] = {depot['id']: [] for depot in _DEPOTS}
    for delivery in _DELIVERIES:
        nearest = min(_DEPOTS, key=lambda depot: _haversine(depot, delivery))
        assigned[nearest['id']].append(delivery)
    routes: dict[str, list[str]] = {}
    total = 0.0
    for depot in _DEPOTS:
        remaining = list(assigned[depot['id']])
        current = depot
        order: list[str] = []
        while remaining:
            nxt = min(remaining, key=lambda stop: _haversine(current, stop))
            total += _haversine(current, nxt)
            order.append(nxt['id'])
            remaining.remove(nxt)
            current = nxt
        routes[depot['id']] = order
    return {'routes': routes, 'total_km': round(total, 1)}


REFERENCE = """
import asyncio
import math

depots, deliveries = await asyncio.gather(fetch_depots(), fetch_deliveries())

def haversine(a, b):
    lat1 = math.radians(a['lat'])
    lat2 = math.radians(b['lat'])
    dlat = lat2 - lat1
    dlng = math.radians(b['lng'] - a['lng'])
    h = math.sin(dlat / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin(dlng / 2) ** 2
    return 2 * EARTH_RADIUS_KM * math.atan2(math.sqrt(h), math.sqrt(1 - h))

assigned = {}
for depot in depots:
    assigned[depot['id']] = []
for delivery in deliveries:
    best = depots[0]['id']
    best_km = haversine(depots[0], delivery)
    for depot in depots[1:]:
        km = haversine(depot, delivery)
        if km < best_km:
            best = depot['id']
            best_km = km
    assigned[best].append(delivery)

routes = {}
total = 0.0
for depot in depots:
    remaining = list(assigned[depot['id']])
    current = depot
    order = []
    while remaining:
        nxt = remaining[0]
        nxt_km = haversine(current, nxt)
        for stop in remaining[1:]:
            km = haversine(current, stop)
            if km < nxt_km:
                nxt = stop
                nxt_km = km
        total = total + nxt_km
        order.append(nxt['id'])
        remaining = [stop for stop in remaining if stop['id'] != nxt['id']]
        current = nxt
    routes[depot['id']] = order

{'routes': routes, 'total_km': round(total, 1)}
"""

TASK = Task(
    name='haversine_routes',
    category='planning',
    prompt=(
        'Assign every delivery to its nearest depot by great-circle (haversine) distance using '
        "EARTH_RADIUS_KM, then order each depot's deliveries as a nearest-neighbour route: "
        'start at the depot, repeatedly go to the closest remaining stop. Return a dict with '
        '"routes" (depot id to the ordered list of delivery ids, every depot present even if '
        'empty) and "total_km" (the sum of all route legs from each depot through its stops, '
        'rounded to 1 decimal place).'
    ),
    stubs=STUBS,
    tools={'fetch_depots': fetch_depots, 'fetch_deliveries': fetch_deliveries},
    inputs={'EARTH_RADIUS_KM': EARTH_RADIUS_KM},
    expected=_expected(),
    evaluators=(ApproxExpected(),),
    reference_solution=REFERENCE,
    traps=('math.radians/atan2', 'min with key', 'geopy reflex'),
    expected_external_calls=2,
    expected_call_batches=1,
)
