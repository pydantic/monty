"""Price an option book with Black-Scholes and back out implied volatilities.

The normal CDF has to come from `math.erf`, and the implied volatilities need a
Newton iteration on the pricing function, so the case exercises the analytic end
of `math` (`erf`, `exp`, `log`, `sqrt`) rather than plain arithmetic.
"""

from __future__ import annotations

import asyncio
import math
from typing import Any

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

_BOOK: list[dict[str, Any]] = [
    {'id': 'C1', 'kind': 'call', 'spot': 100.0, 'strike': 95.0, 'rate': 0.03, 'vol': 0.22, 'expiry': 0.5},
    {'id': 'C2', 'kind': 'call', 'spot': 100.0, 'strike': 105.0, 'rate': 0.03, 'vol': 0.22, 'expiry': 0.5},
    {'id': 'C3', 'kind': 'call', 'spot': 250.0, 'strike': 260.0, 'rate': 0.04, 'vol': 0.35, 'expiry': 1.0},
    {'id': 'C4', 'kind': 'call', 'spot': 42.0, 'strike': 40.0, 'rate': 0.05, 'vol': 0.18, 'expiry': 0.25},
    {'id': 'C5', 'kind': 'call', 'spot': 1800.0, 'strike': 1900.0, 'rate': 0.02, 'vol': 0.28, 'expiry': 2.0},
    {'id': 'C6', 'kind': 'call', 'spot': 15.0, 'strike': 15.0, 'rate': 0.01, 'vol': 0.6, 'expiry': 0.75},
    {'id': 'P1', 'kind': 'put', 'spot': 100.0, 'strike': 95.0, 'rate': 0.03, 'vol': 0.22, 'expiry': 0.5},
    {'id': 'P2', 'kind': 'put', 'spot': 100.0, 'strike': 105.0, 'rate': 0.03, 'vol': 0.22, 'expiry': 0.5},
    {'id': 'P3', 'kind': 'put', 'spot': 250.0, 'strike': 240.0, 'rate': 0.04, 'vol': 0.35, 'expiry': 1.0},
    {'id': 'P4', 'kind': 'put', 'spot': 42.0, 'strike': 45.0, 'rate': 0.05, 'vol': 0.18, 'expiry': 0.25},
    {'id': 'P5', 'kind': 'put', 'spot': 1800.0, 'strike': 1700.0, 'rate': 0.02, 'vol': 0.28, 'expiry': 2.0},
    {'id': 'P6', 'kind': 'put', 'spot': 15.0, 'strike': 12.0, 'rate': 0.01, 'vol': 0.6, 'expiry': 0.75},
]

HOST_LATENCY = 0.005
"""Simulated round trip per host call, so gathered fetches overlap and `call_batches` can see them."""

_QUOTES: dict[str, float] = {'C1': 9.85, 'P3': 24.4, 'C5': 310.0}
"""Market prices whose implied volatility is asked for."""


async def fetch_book() -> list[dict[str, Any]]:
    """Host function: the option book."""
    await asyncio.sleep(HOST_LATENCY)
    return [dict(option) for option in _BOOK]


async def fetch_quotes() -> dict[str, float]:
    """Host function: market prices by option id."""
    await asyncio.sleep(HOST_LATENCY)
    return dict(_QUOTES)


STUBS = '''
from typing import Any

async def fetch_book() -> list[dict[str, Any]]:
    """Return the option book.

    Each option has `id`, `kind` (`"call"` or `"put"`), `spot`, `strike`, `rate`
    (continuously compounded, annual), `vol` (annual) and `expiry` (years).
    """
    ...

async def fetch_quotes() -> dict[str, float]:
    """Return market prices keyed by option id, for the implied-volatility part."""
    ...
'''


def _norm_cdf(x: float) -> float:
    return 0.5 * (1.0 + math.erf(x / math.sqrt(2.0)))


def _price(kind: str, spot: float, strike: float, rate: float, vol: float, expiry: float) -> float:
    d1 = (math.log(spot / strike) + (rate + 0.5 * vol * vol) * expiry) / (vol * math.sqrt(expiry))
    d2 = d1 - vol * math.sqrt(expiry)
    if kind == 'call':
        return spot * _norm_cdf(d1) - strike * math.exp(-rate * expiry) * _norm_cdf(d2)
    return strike * math.exp(-rate * expiry) * _norm_cdf(-d2) - spot * _norm_cdf(-d1)


def _vega(spot: float, strike: float, rate: float, vol: float, expiry: float) -> float:
    d1 = (math.log(spot / strike) + (rate + 0.5 * vol * vol) * expiry) / (vol * math.sqrt(expiry))
    return spot * math.sqrt(expiry) * math.exp(-0.5 * d1 * d1) / math.sqrt(2.0 * math.pi)


def _implied_vol(option: dict[str, Any], target: float) -> float:
    vol = 0.2
    for _ in range(50):
        diff = _price(option['kind'], option['spot'], option['strike'], option['rate'], vol, option['expiry']) - target
        if abs(diff) < 1e-8:
            break
        vol -= diff / _vega(option['spot'], option['strike'], option['rate'], vol, option['expiry'])
    return vol


def _expected() -> dict[str, dict[str, float]]:
    by_id = {option['id']: option for option in _BOOK}
    prices = {
        o['id']: round(_price(o['kind'], o['spot'], o['strike'], o['rate'], o['vol'], o['expiry']), 4) for o in _BOOK
    }
    vols = {oid: round(_implied_vol(by_id[oid], quote), 4) for oid, quote in _QUOTES.items()}
    return {'prices': prices, 'implied_vols': vols}


REFERENCE = """
import asyncio
import math

book, quotes = await asyncio.gather(fetch_book(), fetch_quotes())

def norm_cdf(x):
    return 0.5 * (1.0 + math.erf(x / math.sqrt(2.0)))

def d1_d2(spot, strike, rate, vol, expiry):
    d1 = (math.log(spot / strike) + (rate + 0.5 * vol * vol) * expiry) / (vol * math.sqrt(expiry))
    return d1, d1 - vol * math.sqrt(expiry)

def price(kind, spot, strike, rate, vol, expiry):
    d1, d2 = d1_d2(spot, strike, rate, vol, expiry)
    if kind == 'call':
        return spot * norm_cdf(d1) - strike * math.exp(-rate * expiry) * norm_cdf(d2)
    return strike * math.exp(-rate * expiry) * norm_cdf(-d2) - spot * norm_cdf(-d1)

def vega(spot, strike, rate, vol, expiry):
    d1, _ = d1_d2(spot, strike, rate, vol, expiry)
    return spot * math.sqrt(expiry) * math.exp(-0.5 * d1 * d1) / math.sqrt(2.0 * math.pi)

prices = {}
by_id = {}
for o in book:
    by_id[o['id']] = o
    prices[o['id']] = round(price(o['kind'], o['spot'], o['strike'], o['rate'], o['vol'], o['expiry']), 4)

implied = {}
for oid, target in quotes.items():
    o = by_id[oid]
    vol = 0.2
    for _ in range(50):
        diff = price(o['kind'], o['spot'], o['strike'], o['rate'], vol, o['expiry']) - target
        if abs(diff) < 1e-8:
            break
        vol = vol - diff / vega(o['spot'], o['strike'], o['rate'], vol, o['expiry'])
    implied[oid] = round(vol, 4)

{'prices': prices, 'implied_vols': implied}
"""

TASK = Task(
    name='black_scholes',
    category='numeric',
    prompt=(
        'Price every option in the book with the Black-Scholes formula (no dividends, '
        'continuously compounded rate, normal CDF from math.erf). Then, for each option in '
        'the quotes, find the implied volatility that reproduces the quoted price by Newton '
        'iteration on volatility using the analytic vega, starting from 0.2 and stopping when '
        'the price difference is below 1e-8. Return a dict with "prices" (option id to price, '
        'rounded to 4 decimal places) and "implied_vols" (option id to volatility, rounded to '
        '4 decimal places).'
    ),
    stubs=STUBS,
    tools={'fetch_book': fetch_book, 'fetch_quotes': fetch_quotes},
    expected=_expected(),
    evaluators=(ApproxExpected(),),
    reference_solution=REFERENCE,
    traps=('math.erf', 'Newton iteration', 'scipy.stats reflex'),
    expected_external_calls=2,
    expected_call_batches=1,
)
