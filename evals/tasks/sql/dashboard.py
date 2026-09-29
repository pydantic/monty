"""A six-panel report from a small warehouse: SQL, a transform, and one chart per panel.

Each panel needs its own query and a different reshaping in Python (monthly sums,
a region-by-quarter pivot, a weekday-by-month grid, per-product joins, binning,
shares). The recorder keeps every `draw_chart` call, so the evaluator checks the
chart kinds and a handful of exact data points rather than pixels.
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
from pydantic_monty import MountDir

OUTPUT_DIR = Path(__file__).parent.parent.parent / 'reports' / 'artifacts' / 'dashboard'
OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
REPORT_PATH = OUTPUT_DIR / 'report.md'

_REGIONS = ['EMEA', 'AMER', 'APAC']
_PRODUCTS = [
    (1, 'Widget', 'hardware', 12.0),
    (2, 'Gadget', 'hardware', 30.0),
    (3, 'Gizmo', 'hardware', 55.0),
    (4, 'Support plan', 'services', 80.0),
    (5, 'Training', 'services', 140.0),
    (6, 'Licence', 'software', 25.0),
    (7, 'Add-on', 'software', 9.0),
]
_MONTHS = [f'2025-{m:02d}' for m in range(1, 13)]
_WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']
_BINS = [0, 100, 200, 300, 400, 500, 600, 700, 800, 900]


def _build_orders() -> list[dict[str, Any]]:
    rng = random.Random(20250101)
    orders: list[dict[str, Any]] = []
    for order_id in range(1, 1201):
        day = date(2025, 1, 1) + timedelta(days=rng.randrange(365))
        product = rng.choice(_PRODUCTS)
        quantity = rng.randrange(1, 9)
        orders.append(
            {
                'id': order_id,
                'order_date': day.isoformat(),
                'region': rng.choice(_REGIONS),
                'product_id': product[0],
                'quantity': quantity,
                'amount': round(product[3] * quantity * rng.uniform(0.9, 1.3), 2),
            }
        )
    return orders


_ORDERS = _build_orders()


def _seed(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, category TEXT, unit_cost REAL);
        CREATE TABLE regions (name TEXT PRIMARY KEY, country TEXT);
        CREATE TABLE orders (id INTEGER PRIMARY KEY, order_date TEXT, region TEXT, product_id INTEGER,
                             quantity INTEGER, amount REAL);
        """
    )
    conn.executemany('INSERT INTO products VALUES (?, ?, ?, ?)', _PRODUCTS)
    conn.executemany('INSERT INTO regions VALUES (?, ?)', [('EMEA', 'GB'), ('AMER', 'US'), ('APAC', 'SG')])
    conn.executemany('INSERT INTO orders VALUES (:id, :order_date, :region, :product_id, :quantity, :amount)', _ORDERS)


DB = SqliteDb(_seed, read_only=True)
CHARTS = ChartRecorder(OUTPUT_DIR / 'charts')


def _facts() -> dict[str, Any]:
    """The numbers the panels must show, computed from the fixture."""
    monthly = {m: 0.0 for m in _MONTHS}
    by_region_quarter = {r: [0.0, 0.0, 0.0, 0.0] for r in _REGIONS}
    grid = {d: [0] * 12 for d in _WEEKDAYS}
    per_product = {p[0]: 0 for p in _PRODUCTS}
    bins = [0] * len(_BINS)
    by_category: dict[str, float] = {}
    category = {p[0]: p[2] for p in _PRODUCTS}
    for order in _ORDERS:
        day = date.fromisoformat(order['order_date'])
        monthly[order['order_date'][:7]] += order['amount']
        by_region_quarter[order['region']][(day.month - 1) // 3] += order['amount']
        grid[_WEEKDAYS[day.weekday()]][day.month - 1] += 1
        per_product[order['product_id']] += order['quantity']
        bins[min(int(order['amount'] // 100), len(_BINS) - 1)] += 1
        by_category[category[order['product_id']]] = (
            by_category.get(category[order['product_id']], 0.0) + order['amount']
        )
    total = sum(monthly.values())
    return {
        'monthly': [round(monthly[m], 2) for m in _MONTHS],
        'region_quarter': {r: [round(v, 2) for v in vals] for r, vals in by_region_quarter.items()},
        'grid': grid,
        'per_product': per_product,
        'bins': bins,
        'by_category': {c: round(v, 2) for c, v in by_category.items()},
        'total': round(total, 2),
        'top_category': max(by_category, key=lambda c: by_category[c]),
    }


FACTS = _facts()
EXPECTED = {'total_revenue': FACTS['total'], 'top_category': FACTS['top_category']}


def _close(a: float, b: float) -> bool:
    return abs(a - b) <= max(0.05, 0.001 * abs(b))


def _panels_ok(_result: object) -> bool:
    """Six named panels of the right kinds, each carrying the fixture's numbers, and a report linking them."""
    calls = {c.name: c for c in CHARTS.calls}
    wanted = {
        'monthly_revenue': 'line',
        'region_by_quarter': 'stacked_bar',
        'orders_by_weekday': 'heatmap',
        'cost_vs_volume': 'scatter',
        'order_sizes': 'histogram',
        'category_share': 'pie',
    }
    if any(name not in calls or calls[name].kind != kind for name, kind in wanted.items()):
        return False
    monthly = calls['monthly_revenue']
    if len(monthly.x) != 12 or not all(_close(a, b) for a, b in zip(monthly.y, FACTS['monthly'], strict=False)):
        return False
    series: dict[str, list[float]] = calls['region_by_quarter'].options.get('series') or {}
    if set(series) != set(_REGIONS) or not all(
        _close(a, b) for r in _REGIONS for a, b in zip(series[r], FACTS['region_quarter'][r], strict=False)
    ):
        return False
    grid: dict[str, list[int]] = calls['orders_by_weekday'].options.get('series') or {}
    if set(grid) != set(_WEEKDAYS) or any(list(grid[d]) != FACTS['grid'][d] for d in _WEEKDAYS):
        return False
    scatter = calls['cost_vs_volume']
    if sorted(zip(scatter.x, scatter.y, strict=False)) != sorted(
        (float(p[3]), FACTS['per_product'][p[0]]) for p in _PRODUCTS
    ):
        return False
    histogram = calls['order_sizes']
    if list(histogram.x) != _BINS or list(histogram.y) != FACTS['bins']:
        return False
    pie = calls['category_share']
    shares = dict(zip(pie.x, pie.y, strict=False))
    if set(shares) != set(FACTS['by_category']) or not all(
        _close(shares[c], FACTS['by_category'][c]) for c in FACTS['by_category']
    ):
        return False
    if not REPORT_PATH.is_file():
        return False
    report = REPORT_PATH.read_text()
    return all(name in report for name in wanted)


def _reset() -> None:
    DB.reset()
    CHARTS.reset()
    REPORT_PATH.unlink(missing_ok=True)


STUBS = '''
from typing import Any

def query(sql: str) -> list[dict[str, Any]]:
    """Run a read-only SQL query; rows as dicts, or `[{"error": ...}]` when it fails."""
    ...

def list_tables() -> list[str]: ...

def describe_table(name: str) -> list[dict[str, Any]]: ...

async def draw_chart(
    x: list[Any],
    y: list[Any],
    *,
    name: str,
    kind: str = 'line',
    title: str | None = None,
    x_label: str | None = None,
    y_label: str | None = None,
    label: str | None = None,
    series: dict[str, list[Any]] | None = None,
) -> str:
    """Draw one chart and return the path of the image written for it.

    `kind` is one of line, bar, stacked_bar, scatter, histogram, heatmap, pie. For
    stacked_bar and heatmap pass the categories as `x`, the row labels as `y`, and one
    list per row in `series`, keyed by row label.
    """
    ...
'''

PROMPT = """
Build the 2025 sales dashboard from the warehouse: `orders(id, order_date, region, product_id, quantity, amount)`,
`products(id, name, category, unit_cost)` and `regions(name, country)`. Draw exactly these six panels with
`draw_chart`, using the given `name` and `kind`:

1. `monthly_revenue`, line: x = the twelve months as `YYYY-MM`, y = revenue (sum of amount) per month.
2. `region_by_quarter`, stacked_bar: x = `['Q1', 'Q2', 'Q3', 'Q4']`, y = the region names, series = revenue per
   quarter for each region keyed by region.
3. `orders_by_weekday`, heatmap: x = the twelve months, y = `['Mon', ..., 'Sun']`, series = order counts per month for
   each weekday keyed by weekday name.
4. `cost_vs_volume`, scatter: one point per product, x = unit_cost, y = total quantity sold.
5. `order_sizes`, histogram: x = bin lower edges `[0, 100, ..., 900]`, y = number of orders whose amount falls in
   `[edge, edge + 100)`, with everything from 900 up in the last bin.
6. `category_share`, pie: x = product categories, y = revenue per category.

Then write `/output/report.md` with a heading per panel that names it and links the image path `draw_chart` returned.
Return a dict with "total_revenue" (all orders, rounded to 2 decimal places) and "top_category" (the category with
the most revenue).
"""

REFERENCE = """
from pathlib import Path

months = [f'2025-{m:02d}' for m in range(1, 13)]
weekdays = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

monthly = query("SELECT substr(order_date, 1, 7) AS month, SUM(amount) AS revenue FROM orders GROUP BY month ORDER BY month")
revenue_by_month = {row['month']: row['revenue'] for row in monthly}
paths = {}
paths['monthly_revenue'] = await draw_chart(
    months, [round(revenue_by_month.get(m, 0.0), 2) for m in months], name='monthly_revenue', kind='line', title='Monthly revenue'
)

rq = query("SELECT region, (CAST(substr(order_date, 6, 2) AS INTEGER) - 1) / 3 AS q, SUM(amount) AS revenue FROM orders GROUP BY region, q")
regions = sorted({row['region'] for row in rq})
series = {r: [0.0, 0.0, 0.0, 0.0] for r in regions}
for row in rq:
    series[row['region']][row['q']] = round(row['revenue'], 2)
paths['region_by_quarter'] = await draw_chart(
    ['Q1', 'Q2', 'Q3', 'Q4'], regions, name='region_by_quarter', kind='stacked_bar', series=series, title='Revenue by region'
)

days = query("SELECT order_date, COUNT(*) AS n FROM orders GROUP BY order_date")
from datetime import date
grid = {d: [0] * 12 for d in weekdays}
for row in days:
    day = date.fromisoformat(row['order_date'])
    grid[weekdays[day.weekday()]][day.month - 1] += row['n']
paths['orders_by_weekday'] = await draw_chart(
    months, weekdays, name='orders_by_weekday', kind='heatmap', series=grid, title='Orders by weekday and month'
)

pp = query("SELECT p.unit_cost AS cost, SUM(o.quantity) AS units FROM orders o JOIN products p ON o.product_id = p.id GROUP BY p.id")
paths['cost_vs_volume'] = await draw_chart(
    [row['cost'] for row in pp], [row['units'] for row in pp], name='cost_vs_volume', kind='scatter', x_label='unit cost', y_label='units'
)

edges = [0, 100, 200, 300, 400, 500, 600, 700, 800, 900]
counts = [0] * len(edges)
for row in query('SELECT amount FROM orders'):
    index = int(row['amount'] // 100)
    if index > len(edges) - 1:
        index = len(edges) - 1
    counts[index] += 1
paths['order_sizes'] = await draw_chart(edges, counts, name='order_sizes', kind='histogram', title='Order sizes')

cats = query("SELECT p.category AS category, SUM(o.amount) AS revenue FROM orders o JOIN products p ON o.product_id = p.id GROUP BY p.category")
paths['category_share'] = await draw_chart(
    [row['category'] for row in cats], [round(row['revenue'], 2) for row in cats], name='category_share', kind='pie'
)

lines = ['# 2025 sales dashboard', '']
for name in ['monthly_revenue', 'region_by_quarter', 'orders_by_weekday', 'cost_vs_volume', 'order_sizes', 'category_share']:
    lines.append(f'## {name}')
    lines.append('')
    lines.append(f'![{name}]({paths[name]})')
    lines.append('')
Path('/output/report.md').write_text('\\n'.join(lines))

total = 0.0
for row in cats:
    total = total + row['revenue']
top = sorted(cats, key=lambda row: row['revenue'], reverse=True)[0]['category']
{'total_revenue': round(total, 2), 'top_category': top}
"""

TASK = Task(
    name='dashboard',
    category='sql',
    prompt=PROMPT.strip(),
    stubs=STUBS,
    tools={
        'query': DB.query,
        'list_tables': DB.list_tables,
        'describe_table': DB.describe_table,
        'draw_chart': CHARTS.draw_chart,
    },
    mounts=[MountDir(host_path=OUTPUT_DIR, virtual_path='/output', mode='read-write')],
    expected=EXPECTED,
    evaluators=(
        ApproxExpected(),
        Predicate('six panels drawn with the right kinds and data, report written', _panels_ok),
    ),
    reference_solution=REFERENCE,
    traps=('date.weekday from a string', 'binning with an open last bin', 'pivoting for stacked_bar and heatmap'),
    max_result_bytes=120,
    setup=_reset,
)
