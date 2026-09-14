"""Quote a multi-currency order with tiered pricing, dated tax rates and FX.

Money must be rounded half up to cents at every line, which `decimal` would do in
one call; Monty has no `decimal`, so the reference truncates `x * 100 + 0.5`.
Tax rates change on a date, so the order date picks the rate with `datetime`.
"""

from __future__ import annotations

import asyncio
from datetime import date
from typing import Any

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

ORDER_DATE = '2026-04-15'
HOST_LATENCY = 0.005
"""Simulated round trip per host call, so gathered fetches overlap and `call_batches` can see them."""

_TIERS: dict[str, list[dict[str, Any]]] = {
    'WIDGET': [{'min_qty': 1, 'unit': 4.99}, {'min_qty': 50, 'unit': 4.49}, {'min_qty': 200, 'unit': 3.99}],
    'GADGET': [{'min_qty': 1, 'unit': 27.5}, {'min_qty': 10, 'unit': 25.0}],
    'GIZMO': [{'min_qty': 1, 'unit': 0.35}, {'min_qty': 1000, 'unit': 0.29}],
}

_TAX: dict[str, list[dict[str, Any]]] = {
    'GB': [{'from': '2020-01-01', 'rate': 0.2}],
    'DE': [{'from': '2020-01-01', 'rate': 0.19}, {'from': '2026-04-01', 'rate': 0.21}],
    'US': [{'from': '2020-01-01', 'rate': 0.0725}, {'from': '2026-07-01', 'rate': 0.08}],
    'FR': [{'from': '2020-01-01', 'rate': 0.2}],
}

_FX_TO_USD = {'USD': 1.0, 'EUR': 1.0847, 'GBP': 1.2731}

_LINES: list[dict[str, Any]] = [
    {'sku': 'WIDGET', 'qty': 75, 'currency': 'GBP', 'country': 'GB'},
    {'sku': 'GADGET', 'qty': 3, 'currency': 'EUR', 'country': 'DE'},
    {'sku': 'GIZMO', 'qty': 1500, 'currency': 'USD', 'country': 'US'},
    {'sku': 'WIDGET', 'qty': 250, 'currency': 'EUR', 'country': 'FR'},
    {'sku': 'GADGET', 'qty': 12, 'currency': 'USD', 'country': 'US'},
    {'sku': 'GIZMO', 'qty': 999, 'currency': 'GBP', 'country': 'GB'},
    {'sku': 'WIDGET', 'qty': 49, 'currency': 'EUR', 'country': 'DE'},
]


async def fetch_order() -> dict[str, Any]:
    """Host function: the order lines and date."""
    await asyncio.sleep(HOST_LATENCY)
    return {'date': ORDER_DATE, 'lines': [dict(line) for line in _LINES]}


async def fetch_price_tiers() -> dict[str, list[dict[str, Any]]]:
    """Host function: unit price by sku and minimum quantity."""
    await asyncio.sleep(HOST_LATENCY)
    return {sku: [dict(tier) for tier in tiers] for sku, tiers in _TIERS.items()}


async def fetch_tax_rates() -> dict[str, list[dict[str, Any]]]:
    """Host function: tax rates by country, each effective from a date."""
    await asyncio.sleep(HOST_LATENCY)
    return {country: [dict(rate) for rate in rates] for country, rates in _TAX.items()}


async def fetch_fx_rates() -> dict[str, float]:
    """Host function: USD per unit of each currency."""
    await asyncio.sleep(HOST_LATENCY)
    return dict(_FX_TO_USD)


STUBS = '''
from typing import Any

async def fetch_order() -> dict[str, Any]:
    """Return `{"date": "YYYY-MM-DD", "lines": [...]}`; each line has `sku`, `qty`, `currency`, `country`."""
    ...

async def fetch_price_tiers() -> dict[str, list[dict[str, Any]]]:
    """Return, per sku, tiers `{"min_qty": int, "unit": float}` ascending; the unit price is the highest tier at or below qty."""
    ...

async def fetch_tax_rates() -> dict[str, list[dict[str, Any]]]:
    """Return, per country, `{"from": "YYYY-MM-DD", "rate": float}` entries ascending by date."""
    ...

async def fetch_fx_rates() -> dict[str, float]:
    """Return USD per unit of each currency, e.g. `{"EUR": 1.08}`."""
    ...
'''


def _half_up(value: float) -> float:
    return int(value * 100 + 0.5) / 100


def _unit_price(sku: str, qty: int) -> float:
    return max((tier for tier in _TIERS[sku] if qty >= tier['min_qty']), key=lambda tier: tier['min_qty'])['unit']


def _tax_rate(country: str, when: date) -> float:
    return max(
        (entry for entry in _TAX[country] if date.fromisoformat(entry['from']) <= when), key=lambda e: e['from']
    )['rate']


def _expected() -> dict[str, Any]:
    when = date.fromisoformat(ORDER_DATE)
    lines: list[dict[str, Any]] = []
    by_currency: dict[str, float] = {}
    total_usd = 0.0
    for line in _LINES:
        net = _half_up(_unit_price(line['sku'], line['qty']) * line['qty'])
        tax = _half_up(net * _tax_rate(line['country'], when))
        gross = _half_up(net + tax)
        usd = _half_up(gross * _FX_TO_USD[line['currency']])
        lines.append({'sku': line['sku'], 'net': net, 'tax': tax, 'gross': gross, 'usd': usd})
        by_currency[line['currency']] = _half_up(by_currency.get(line['currency'], 0.0) + gross)
        total_usd = _half_up(total_usd + usd)
    return {'lines': lines, 'by_currency': by_currency, 'total_usd': total_usd}


REFERENCE = """
import asyncio
from datetime import date

order, tiers, taxes, fx = await asyncio.gather(fetch_order(), fetch_price_tiers(), fetch_tax_rates(), fetch_fx_rates())

def half_up(value):
    return int(value * 100 + 0.5) / 100

when = date.fromisoformat(order['date'])
lines = []
by_currency = {}
total_usd = 0.0
for line in order['lines']:
    unit = None
    for tier in tiers[line['sku']]:
        if line['qty'] >= tier['min_qty']:
            unit = tier['unit']
    rate = None
    for entry in taxes[line['country']]:
        if date.fromisoformat(entry['from']) <= when:
            rate = entry['rate']
    net = half_up(unit * line['qty'])
    tax = half_up(net * rate)
    gross = half_up(net + tax)
    usd = half_up(gross * fx[line['currency']])
    lines.append({'sku': line['sku'], 'net': net, 'tax': tax, 'gross': gross, 'usd': usd})
    by_currency[line['currency']] = half_up(by_currency.get(line['currency'], 0.0) + gross)
    total_usd = half_up(total_usd + usd)

{'lines': lines, 'by_currency': by_currency, 'total_usd': total_usd}
"""

TASK = Task(
    name='quote_builder',
    category='business',
    prompt=(
        'Quote the order. For each line: unit price is the tier with the highest min_qty at '
        "or below the quantity; net = unit * qty; the tax rate is the country's entry with the "
        'latest "from" date on or before the order date; tax = net * rate; gross = net + tax; '
        "usd = gross * the currency's FX rate. Round every one of those money amounts half up "
        'to cents, meaning int(value * 100 + 0.5) / 100, before using it in the next step. '
        'Return {"lines": [{"sku", "net", "tax", "gross", "usd"} in order], "by_currency": '
        '{currency: sum of gross, rounded the same way after each addition}, "total_usd": sum '
        'of usd rounded the same way after each addition}.'
    ),
    stubs=STUBS,
    tools={
        'fetch_order': fetch_order,
        'fetch_price_tiers': fetch_price_tiers,
        'fetch_tax_rates': fetch_tax_rates,
        'fetch_fx_rates': fetch_fx_rates,
    },
    expected=_expected(),
    evaluators=(ApproxExpected(rel_tol=1e-9, abs_tol=0.005),),
    reference_solution=REFERENCE,
    traps=('decimal.ROUND_HALF_UP', 'round() is half-even', 'tax rate effective by date'),
    expected_external_calls=4,
    expected_call_batches=1,
)
