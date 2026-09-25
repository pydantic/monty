"""Reconcile bank lines against invoices with amount and date tolerances.

Two passes the prompt pins exactly: exact amount within three days, then amount
within 1% when the bank reference carries the invoice number. The fixture has
fees, unpaid invoices and short payments so every branch is exercised, and the
expected report is computed host-side by the same rules.
"""

from __future__ import annotations

import asyncio
import re
from datetime import date, timedelta
from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.task import Task

_CUSTOMERS = ['Acme Ltd', 'Bletchley Park', 'Compiler Works', 'Dijkstra BV', 'Erlang Systems', 'Fermat Finance']
_START = date(2026, 3, 2)
HOST_LATENCY = 0.005
"""Simulated round trip per host call, so gathered fetches overlap and `call_batches` can see them."""


def _build() -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """55 invoices and 60 bank lines: exact pays, late pays, short pays, fees and unpaid invoices."""
    invoices: list[dict[str, Any]] = []
    bank: list[dict[str, Any]] = []
    seed = 20260302
    for index in range(55):
        seed = (seed * 1103515245 + 12345) % 2**31
        amount = round(120 + (seed % 90000) / 100, 2)
        issued = _START + timedelta(days=index)
        number = f'INV-{2600 + index}'
        invoices.append(
            {
                'id': number,
                'date': issued.isoformat(),
                'amount': amount,
                'customer': _CUSTOMERS[index % len(_CUSTOMERS)],
            }
        )
        kind = index % 11
        if kind == 4:
            continue  # unpaid
        if kind == 7:
            paid_amount = round(amount * 0.995, 2)  # short payment, within 1%
            reference = f'PAYMENT {number} LESS CHARGES'
            paid = issued + timedelta(days=5)
        elif kind == 9:
            paid_amount = amount
            reference = f'{_CUSTOMERS[index % len(_CUSTOMERS)].upper()} REF {number.replace("-", "")}'
            paid = issued + timedelta(days=3)
        else:
            paid_amount = amount
            reference = f'TRF {number}'
            paid = issued + timedelta(days=index % 4)
        bank.append(
            {'id': f'B{len(bank) + 1:03d}', 'date': paid.isoformat(), 'amount': paid_amount, 'reference': reference}
        )
    for index in range(10):
        fee_day = _START + timedelta(days=6 * index)
        bank.append(
            {
                'id': f'B{len(bank) + 1:03d}',
                'date': fee_day.isoformat(),
                'amount': -12.5,
                'reference': f'BANK FEE {index}',
            }
        )
    bank.sort(key=lambda line: (line['date'], line['id']))
    return invoices, bank


_INVOICES, _BANK = _build()


async def fetch_bank_lines() -> list[dict[str, Any]]:
    """Host function: the bank statement lines."""
    await asyncio.sleep(HOST_LATENCY)
    return [dict(line) for line in _BANK]


async def fetch_invoices() -> list[dict[str, Any]]:
    """Host function: the invoices awaiting payment."""
    await asyncio.sleep(HOST_LATENCY)
    return [dict(invoice) for invoice in _INVOICES]


STUBS = '''
from typing import Any

async def fetch_bank_lines() -> list[dict[str, Any]]:
    """Return bank statement lines: `id`, `date` (YYYY-MM-DD), `amount`, `reference` (free text)."""
    ...

async def fetch_invoices() -> list[dict[str, Any]]:
    """Return invoices: `id` like `INV-2600`, `date` (YYYY-MM-DD), `amount`, `customer`."""
    ...
'''


def _expected() -> dict[str, Any]:
    unmatched_invoices = sorted(_INVOICES, key=lambda inv: (inv['date'], inv['id']))
    matches: list[list[str]] = []
    unmatched_bank: list[str] = []
    for line in _BANK:
        line_date = date.fromisoformat(line['date'])
        exact = [
            inv
            for inv in unmatched_invoices
            if inv['amount'] == line['amount'] and abs((line_date - date.fromisoformat(inv['date'])).days) <= 3
        ]
        if exact:
            chosen = exact[0]
        else:
            digits = re.sub(r'\D', '', line['reference'])
            fuzzy = [
                inv
                for inv in unmatched_invoices
                if abs(line['amount'] - inv['amount']) <= 0.01 * inv['amount']
                and re.sub(r'\D', '', inv['id']) in digits
            ]
            chosen = fuzzy[0] if fuzzy else None
        if chosen is None:
            unmatched_bank.append(line['id'])
        else:
            matches.append([line['id'], chosen['id']])
            unmatched_invoices.remove(chosen)
    return {
        'matches': matches,
        'unmatched_bank': unmatched_bank,
        'unmatched_invoices': [inv['id'] for inv in unmatched_invoices],
    }


REFERENCE = """
import asyncio
import re
from datetime import date

bank, invoices = await asyncio.gather(fetch_bank_lines(), fetch_invoices())

open_invoices = sorted(invoices, key=lambda inv: (inv['date'], inv['id']))
matches = []
unmatched_bank = []
for line in bank:
    line_date = date.fromisoformat(line['date'])
    chosen = None
    for inv in open_invoices:
        days = (line_date - date.fromisoformat(inv['date'])).days
        if inv['amount'] == line['amount'] and abs(days) <= 3:
            chosen = inv
            break
    if chosen is None:
        digits = re.sub(r'\\D', '', line['reference'])
        for inv in open_invoices:
            close = abs(line['amount'] - inv['amount']) <= 0.01 * inv['amount']
            if close and re.sub(r'\\D', '', inv['id']) in digits:
                chosen = inv
                break
    if chosen is None:
        unmatched_bank.append(line['id'])
    else:
        matches.append([line['id'], chosen['id']])
        open_invoices = [inv for inv in open_invoices if inv['id'] != chosen['id']]

{
    'matches': matches,
    'unmatched_bank': unmatched_bank,
    'unmatched_invoices': [inv['id'] for inv in open_invoices],
}
"""

TASK = Task(
    name='ledger_match',
    category='business',
    prompt=(
        'Reconcile the bank lines against the invoices. Process bank lines in the order given, '
        'against the invoices still unmatched sorted by (date, id). First look for an invoice '
        'with exactly the same amount dated within 3 days either side of the bank line; if '
        'none, look for one whose amount is within 1% of the invoice amount and whose invoice '
        'number digits appear in the digits of the bank reference. Take the first candidate '
        'in each pass and mark it matched. Return {"matches": [[bank_id, invoice_id], ...] in '
        'bank order, "unmatched_bank": [bank ids], "unmatched_invoices": [invoice ids in '
        '(date, id) order]}.'
    ),
    stubs=STUBS,
    tools={'fetch_bank_lines': fetch_bank_lines, 'fetch_invoices': fetch_invoices},
    expected=_expected(),
    evaluators=(EqualsExpected(),),
    reference_solution=REFERENCE,
    traps=('date arithmetic', 're.sub', 'fuzzy tolerance ordering'),
    expected_external_calls=2,
    expected_call_batches=1,
)
