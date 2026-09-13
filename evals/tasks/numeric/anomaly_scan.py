"""Flag anomalous days in a year of hourly metrics pulled in pages.

8,760 hourly rows arrive 1,000 at a time, so the code has to aggregate to daily totals
as it goes rather than hold every row; the statistics are a trailing seven-day
window, a z-score, and an `erf`-based two-sided p-value.
"""

from __future__ import annotations

import math
from datetime import date, timedelta
from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.task import Task

PAGE_SIZE = 1000
P_THRESHOLD = 0.001
_START = date(2025, 1, 1)
_HOURS = 8760
_ANOMALIES = {date(2025, 2, 14): 2.6, date(2025, 5, 3): 0.35, date(2025, 8, 21): 2.2, date(2025, 11, 9): 2.9}
"""Days whose values are scaled, far enough apart that one does not mask the next."""


def _build() -> list[dict[str, Any]]:
    """Baseline with a daily cycle, a weekday effect and small deterministic noise."""
    rows: list[dict[str, Any]] = []
    seed = 12345
    for hour in range(_HOURS):
        day = _START + timedelta(days=hour // 24)
        seed = (seed * 1103515245 + 12345) % 2**31
        noise = (seed / 2**31 - 0.5) * 6
        value = 100 + 20 * math.sin(2 * math.pi * (hour % 24) / 24) + (-8 if day.weekday() >= 5 else 0) + noise
        value *= _ANOMALIES.get(day, 1.0)
        rows.append({'ts': f'{day.isoformat()}T{hour % 24:02d}:00:00', 'value': round(value, 3)})
    return rows


_ROWS = _build()


async def fetch_metrics(page: int = 0) -> dict[str, Any]:
    """Host function: one page of hourly rows and the next page number, or `None` at the end."""
    start = page * PAGE_SIZE
    chunk = _ROWS[start : start + PAGE_SIZE]
    next_page = page + 1 if start + PAGE_SIZE < len(_ROWS) else None
    return {'rows': [dict(row) for row in chunk], 'next_page': next_page}


STUBS = '''
from typing import Any

PAGE_SIZE: int = 1000
P_THRESHOLD: float = 0.001

async def fetch_metrics(page: int = 0) -> dict[str, Any]:
    """Return one page of hourly metric rows, oldest first.

    `rows` is a list of `{"ts": "YYYY-MM-DDTHH:00:00", "value": float}`; `next_page` is
    the page number to fetch next, or `None` after the last page.
    """
    ...
'''


def _expected() -> list[str]:
    totals: dict[str, float] = {}
    for row in _ROWS:
        day = row['ts'][:10]
        totals[day] = totals.get(day, 0.0) + row['value']
    days = sorted(totals)
    flagged: list[str] = []
    for index in range(7, len(days)):
        window = [totals[d] for d in days[index - 7 : index]]
        mean = sum(window) / 7
        variance = sum((v - mean) ** 2 for v in window) / 6
        std = math.sqrt(variance)
        if std == 0:
            continue
        z = (totals[days[index]] - mean) / std
        p = math.erfc(abs(z) / math.sqrt(2))
        if p < P_THRESHOLD:
            flagged.append(days[index])
    return flagged


EXPECTED = _expected()
assert EXPECTED == sorted(d.isoformat() for d in _ANOMALIES), EXPECTED

REFERENCE = """
import math

totals = {}
page = 0
while page is not None:
    result = await fetch_metrics(page=page)
    for row in result['rows']:
        day = row['ts'][:10]
        totals[day] = totals.get(day, 0.0) + row['value']
    page = result['next_page']

days = sorted(totals)
flagged = []
for index in range(7, len(days)):
    window = [totals[d] for d in days[index - 7:index]]
    mean = sum(window) / 7
    variance = 0.0
    for v in window:
        variance = variance + (v - mean) ** 2
    std = math.sqrt(variance / 6)
    if std == 0:
        continue
    z = (totals[days[index]] - mean) / std
    p = math.erfc(abs(z) / math.sqrt(2))
    if p < P_THRESHOLD:
        flagged.append(days[index])

flagged
"""

_PAGES = -(-_HOURS // PAGE_SIZE)

TASK = Task(
    name='anomaly_scan',
    category='numeric',
    prompt=(
        'Fetch every page of hourly metrics and total the values per calendar day (from the '
        'first 10 characters of "ts"). For each day from the eighth onwards, take the previous '
        'seven daily totals, compute their mean and sample standard deviation (divide by 6), '
        "the z-score of the day's total against them, and the two-sided p-value "
        'math.erfc(abs(z) / sqrt(2)). Skip days whose window has zero deviation. Return the '
        'sorted list of dates (YYYY-MM-DD) whose p-value is below P_THRESHOLD. Keep only daily '
        'totals in memory, not the hourly rows.'
    ),
    stubs=STUBS,
    tools={'fetch_metrics': fetch_metrics},
    inputs={'PAGE_SIZE': PAGE_SIZE, 'P_THRESHOLD': P_THRESHOLD},
    expected=EXPECTED,
    evaluators=(EqualsExpected(),),
    reference_solution=REFERENCE,
    traps=('statistics module', 'scipy.stats.norm reflex', 'holding 8,760 rows'),
    expected_external_calls=_PAGES,
    expected_call_batches=_PAGES,
    max_result_bytes=200,
)
