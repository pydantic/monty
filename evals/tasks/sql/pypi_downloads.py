"""Explain a download spike: real usage or a mirror (the Py AI March 2026 demo).

Sixty days of per-installer download counts, with one day where a `bandersnatch`
mirror sync adds forty thousand downloads. The answer is a diagnosis, not a number,
so the code has to look at the breakdown rather than the total; `plot` and
`display_table` are there because the demo used them and the case records that they
were.
"""

from __future__ import annotations

import random
import sqlite3
from datetime import date, timedelta
from pathlib import Path
from typing import Any

from evals.harness.evaluators import ApproxExpected, Predicate
from evals.harness.fixtures import ChartRecorder, SqliteDb
from evals.harness.task import Task

OUTPUT_DIR = Path(__file__).parent.parent.parent / 'reports' / 'artifacts' / 'pypi_downloads'
OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

_INSTALLERS = {'pip': 900, 'uv': 350, 'poetry': 60, 'requests': 40}
_SPIKE_DATE = date(2026, 8, 14)
_SPIKE_ROWS = 40_000
_FIRST = date(2026, 7, 1)


def _rows() -> list[tuple[str, str, str, int]]:
    rng = random.Random(20260814)
    rows: list[tuple[str, str, str, int]] = []
    for offset in range(60):
        day = (_FIRST + timedelta(days=offset)).isoformat()
        for installer, mean in _INSTALLERS.items():
            for version in ('3.12', '3.13', '3.14'):
                rows.append((day, installer, version, max(0, int(rng.gauss(mean / 3, mean / 12)))))
        if _FIRST + timedelta(days=offset) == _SPIKE_DATE:
            rows.append((day, 'bandersnatch', 'unknown', _SPIKE_ROWS))
    return rows


_ROWS = _rows()


def _seed(conn: sqlite3.Connection) -> None:
    conn.execute('CREATE TABLE downloads (date TEXT, installer TEXT, python_version TEXT, count INTEGER)')
    conn.executemany('INSERT INTO downloads VALUES (?, ?, ?, ?)', _ROWS)


DB = SqliteDb(_seed, read_only=True)
CHARTS = ChartRecorder(OUTPUT_DIR / 'charts')
TABLES: list[dict[str, Any]] = []


def _expected() -> dict[str, Any]:
    day_total = sum(count for day, _, _, count in _ROWS if day == _SPIKE_DATE.isoformat())
    return {'spike_date': _SPIKE_DATE.isoformat(), 'cause': 'mirror', 'share': round(_SPIKE_ROWS / day_total, 3)}


EXPECTED = _expected()


async def plot(
    x: list[Any], y: list[Any], *, name: str, kind: str = 'line', title: str | None = None, label: str | None = None
) -> str:
    """Host function: a chart named `name`, recorded for the evaluator."""
    return await CHARTS.draw_chart(x, y, name=name, kind=kind, title=title, label=label)


def display_table(headers: list[str], rows: list[list[Any]], *, title: str | None = None) -> str:
    """Host function: shows a table to the user; recorded for the evaluator."""
    TABLES.append({'headers': headers, 'rows': rows, 'title': title})
    return f'TABLE DISPLAYED: {len(rows)} rows'


def _showed_work(_result: object) -> bool:
    """At least one plot of downloads over time and one table were shown."""
    return bool(CHARTS.calls) and any(len(c.x) >= 30 for c in CHARTS.calls) and bool(TABLES)


def _reset() -> None:
    DB.reset()
    CHARTS.reset()
    TABLES.clear()


STUBS = '''
from typing import Any

def sql_query(sql: str) -> list[dict[str, Any]]:
    """Query the `downloads(date, installer, python_version, count)` table; rows as dicts."""
    ...

async def plot(x: list[Any], y: list[Any], *, name: str, kind: str = 'line', title: str | None = None, label: str | None = None) -> str:
    """Draw a chart for the user (`kind` is line, bar or scatter) and return its file path."""
    ...

def display_table(headers: list[str], rows: list[list[Any]], *, title: str | None = None) -> str:
    """Show a formatted table to the user."""
    ...
'''

PROMPT = """
Downloads of our package jumped recently. Using the `downloads` table (one row per day, installer and Python
version, with a `count`), find the day of the spike and decide whether it was real usage or something else: a
mirror (installers such as `bandersnatch` sync whole indexes) or CI. Plot daily totals over the whole period with
`plot`, show the installer breakdown for the spike day with `display_table`, and return a dict with "spike_date",
"cause" (one of "real", "mirror", "ci") and "share": the fraction of that day's downloads attributable to the cause,
rounded to 3 decimal places.
"""

REFERENCE = """
daily = sql_query('SELECT date, SUM(count) AS total FROM downloads GROUP BY date ORDER BY date')
dates = [row['date'] for row in daily]
totals = [row['total'] for row in daily]
await plot(dates, totals, name='daily_downloads', kind='line', title='Daily downloads')

spike = sorted(daily, key=lambda row: row['total'], reverse=True)[0]
spike_date = spike['date']

breakdown = sql_query(f"SELECT installer, SUM(count) AS total FROM downloads WHERE date = '{spike_date}' GROUP BY installer ORDER BY total DESC")
display_table(['installer', 'downloads'], [[row['installer'], row['total']] for row in breakdown], title=f'Installers on {spike_date}')

day_total = 0
for row in breakdown:
    day_total = day_total + row['total']
top = breakdown[0]
mirrors = {'bandersnatch', 'devpi', 'pypi-mirror'}
if top['installer'] in mirrors:
    cause = 'mirror'
elif top['installer'] in {'requests', 'python-requests', 'ci'}:
    cause = 'ci'
else:
    cause = 'real'

{'spike_date': spike_date, 'cause': cause, 'share': round(top['total'] / day_total, 3)}
"""

TASK = Task(
    name='pypi_downloads',
    category='sql',
    prompt=PROMPT.strip(),
    stubs=STUBS,
    tools={'sql_query': DB.query, 'plot': plot, 'display_table': display_table},
    expected=EXPECTED,
    evaluators=(ApproxExpected(), Predicate('a daily plot and an installer table were shown', _showed_work)),
    reference_solution=REFERENCE,
    traps=('answering from the total alone', 'f-string SQL with quotes'),
    max_result_bytes=120,
    setup=_reset,
)
