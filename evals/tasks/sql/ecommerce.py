"""A business question over an ecommerce database whose documented schema is stale.

The prompt names the acquisition column `channel`; the table calls it
`acquisition_channel`, so the first query comes back as an error dict and the code
has to discover the schema with `describe_table` (the PyData London demo's failure
mode). The task also asks for a write, checked by reading the database afterwards.
"""

from __future__ import annotations

import random
import sqlite3
from datetime import date, timedelta
from typing import Any

from evals.harness.evaluators import ApproxExpected, Predicate
from evals.harness.fixtures import SqliteDb
from evals.harness.task import Task

_CHANNELS = ['organic', 'paid_search', 'referral', 'social', 'email']
_SEGMENTS = ['smb', 'enterprise', 'consumer']
_REGIONS = ['west', 'east', 'south', 'midwest']
_CUSTOMERS = 120


def _build() -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    rng = random.Random(20260605)
    customers: list[dict[str, Any]] = []
    orders: list[dict[str, Any]] = []
    order_id = 1
    for cid in range(1, _CUSTOMERS + 1):
        channel = rng.choice(_CHANNELS)
        created = date(2025, 1, 1) + timedelta(days=rng.randrange(365))
        customers.append(
            {
                'id': cid,
                'name': f'Customer {cid}',
                'segment': rng.choice(_SEGMENTS),
                'region': rng.choice(_REGIONS),
                'acquisition_channel': channel,
                'created_at': created.isoformat(),
            }
        )
        # Referral customers spend more, so the answer is not the biggest channel by headcount.
        mean = 180.0 if channel == 'referral' else 110.0
        for _ in range(rng.randrange(1, 7)):
            orders.append(
                {
                    'id': order_id,
                    'customer_id': cid,
                    'order_date': (created + timedelta(days=rng.randrange(1, 200))).isoformat(),
                    'amount': round(max(5.0, rng.gauss(mean, 40.0)), 2),
                }
            )
            order_id += 1
    return customers, orders


_CUSTOMERS_ROWS, _ORDERS_ROWS = _build()


def _seed(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT, segment TEXT, region TEXT,
                                acquisition_channel TEXT, created_at TEXT);
        CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER, order_date TEXT, amount REAL);
        CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, category TEXT, price REAL,
                               stock_quantity INTEGER, reorder_threshold INTEGER);
        CREATE TABLE reports (id INTEGER PRIMARY KEY AUTOINCREMENT, metric TEXT, value TEXT, detail TEXT);
        """
    )
    conn.executemany(
        'INSERT INTO customers VALUES (:id, :name, :segment, :region, :acquisition_channel, :created_at)',
        _CUSTOMERS_ROWS,
    )
    conn.executemany('INSERT INTO orders VALUES (:id, :customer_id, :order_date, :amount)', _ORDERS_ROWS)
    conn.executemany(
        'INSERT INTO products VALUES (?, ?, ?, ?, ?, ?)',
        [(i, f'Product {i}', 'widgets' if i % 2 else 'gadgets', 20.0 + i, 40 - i, 10) for i in range(1, 11)],
    )


DB = SqliteDb(_seed)


def _expected() -> dict[str, Any]:
    spend: dict[int, float] = {}
    for order in _ORDERS_ROWS:
        spend[order['customer_id']] = spend.get(order['customer_id'], 0.0) + order['amount']
    per_channel: dict[str, list[float]] = {}
    for customer in _CUSTOMERS_ROWS:
        per_channel.setdefault(customer['acquisition_channel'], []).append(spend.get(customer['id'], 0.0))
    averages = {channel: sum(values) / len(values) for channel, values in per_channel.items()}
    best = max(averages, key=lambda c: averages[c])
    return {'success': True, 'result': {'channel': best, 'avg_spend': round(averages[best], 2)}}


EXPECTED = _expected()


def _report_written(_result: object) -> bool:
    """The answer was recorded in `reports` with the channel as its value."""
    rows = DB.query("SELECT metric, value FROM reports WHERE metric = 'best_channel'")
    return len(rows) == 1 and rows[0].get('value') == EXPECTED['result']['channel']


STUBS = '''
from typing import Any

def query(sql: str) -> list[dict[str, Any]]:
    """Run SQL; SELECTs return rows as dicts, writes return `[{"rows_affected": n}]`, failures `[{"error": ...}]`."""
    ...

def list_tables() -> list[str]: ...

def describe_table(name: str) -> list[dict[str, Any]]:
    """Column name, type, nullable and primary_key for each column of `name`."""
    ...

def insert_rows(table: str, rows: list[dict[str, Any]]) -> dict[str, Any]:
    """Insert dicts sharing the same keys; `{"inserted": n}` or `{"error": ...}`."""
    ...

def table_count(table: str) -> int: ...
'''

PROMPT = """
You are a data assistant for an ecommerce team. The documented schema is:
`customers(id, name, segment, region, channel, created_at)`, `orders(id, customer_id, order_date, amount)`,
`products(id, name, category, price, stock_quantity, reorder_threshold)` and `reports(id, metric, value, detail)`.
The documentation may be out of date; if a query fails, check the live schema before retrying.

Which acquisition channel brings in the highest-value customers, measured as the average total spend per customer
acquired through that channel (customers with no orders count as 0)? Record the answer with
`insert_rows('reports', [...])` as one row with metric `best_channel`, value set to the channel name and detail a
short explanation. Return `{"success": True, "result": {"channel": ..., "avg_spend": ...}}` with the average rounded
to 2 decimal places, or `{"success": False, "result": "<what went wrong>"}` if you cannot answer.
"""

REFERENCE = """
first = query('SELECT channel, id FROM customers')
if first and 'error' in first[0]:
    columns = [col['name'] for col in describe_table('customers')]
    channel_col = [c for c in columns if 'channel' in c][0]
else:
    channel_col = 'channel'

customers = query(f'SELECT id, {channel_col} AS channel FROM customers')
orders = query('SELECT customer_id, amount FROM orders')

spend = {}
for order in orders:
    spend[order['customer_id']] = spend.get(order['customer_id'], 0.0) + order['amount']

totals = {}
for customer in customers:
    totals.setdefault(customer['channel'], []).append(spend.get(customer['id'], 0.0))

averages = {channel: sum(values) / len(values) for channel, values in totals.items()}
best = sorted(averages, key=lambda c: averages[c], reverse=True)[0]

insert_rows('reports', [{
    'metric': 'best_channel',
    'value': best,
    'detail': f'average spend per customer {averages[best]:.2f} across {len(totals[best])} customers',
}])

{'success': True, 'result': {'channel': best, 'avg_spend': round(averages[best], 2)}}
"""

TASK = Task(
    name='ecommerce',
    category='sql',
    prompt=PROMPT.strip(),
    stubs=STUBS,
    tools={
        'query': DB.query,
        'list_tables': DB.list_tables,
        'describe_table': DB.describe_table,
        'insert_rows': DB.insert_rows,
        'table_count': DB.table_count,
    },
    expected=EXPECTED,
    evaluators=(ApproxExpected(), Predicate('best_channel row recorded in reports', _report_written)),
    reference_solution=REFERENCE,
    traps=('trusting the documented schema', 'giving up on the error dict', 'averaging orders instead of customers'),
    max_result_bytes=200,
    setup=DB.reset,
)
