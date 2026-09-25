"""NPV, IRR and an amortisation schedule for a set of loans.

Root finding for the IRR, compounding for the NPV, and money rounded to cents at
every schedule step: `decimal` would be the natural tool for the last part, so the
case measures whether the model gets exact cents with `round` alone.
"""

from __future__ import annotations

from typing import Any

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

DISCOUNT_RATE = 0.06
"""Annual discount rate for the NPVs, compounded monthly."""

SCHEDULE_LOAN = 'L2'

_LOANS: list[dict[str, Any]] = [
    {'id': 'L1', 'principal': 12000.0, 'annual_rate': 0.049, 'term_months': 24},
    {'id': 'L2', 'principal': 250000.0, 'annual_rate': 0.0375, 'term_months': 360},
    {'id': 'L3', 'principal': 8500.0, 'annual_rate': 0.129, 'term_months': 36},
    {'id': 'L4', 'principal': 40000.0, 'annual_rate': 0.071, 'term_months': 60},
]


async def fetch_loans() -> list[dict[str, Any]]:
    """Host function: the loan book."""
    return [dict(loan) for loan in _LOANS]


STUBS = '''
from typing import Any

DISCOUNT_RATE: float = 0.06
"""Annual discount rate for NPVs; the monthly rate is DISCOUNT_RATE / 12."""

SCHEDULE_LOAN: str = 'L2'
"""The loan whose amortisation schedule is asked for."""

async def fetch_loans() -> list[dict[str, Any]]:
    """Return the loans: `id`, `principal`, `annual_rate` (nominal, compounded monthly), `term_months`."""
    ...
'''


def _payment(principal: float, annual_rate: float, months: int) -> float:
    r = annual_rate / 12
    return round(principal * r / (1 - (1 + r) ** -months), 2)


def _npv(principal: float, payment: float, months: int) -> float:
    r = DISCOUNT_RATE / 12
    return -principal + sum(payment / (1 + r) ** k for k in range(1, months + 1))


def _irr(principal: float, payment: float, months: int) -> float:
    """Annualised monthly IRR by bisection on the NPV of the lender's cash flows."""

    def npv_at(monthly: float) -> float:
        return -principal + sum(payment / (1 + monthly) ** k for k in range(1, months + 1))

    lo, hi = 0.0, 0.1
    for _ in range(100):
        mid = (lo + hi) / 2
        if npv_at(mid) > 0:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2 * 12


def _schedule(principal: float, annual_rate: float, months: int) -> list[dict[str, float | int]]:
    payment = _payment(principal, annual_rate, months)
    balance = principal
    rows: list[dict[str, float | int]] = []
    for month in range(1, months + 1):
        interest = round(balance * annual_rate / 12, 2)
        if month == months:
            principal_part = round(balance, 2)
            payment_now = round(principal_part + interest, 2)
        else:
            principal_part = round(payment - interest, 2)
            payment_now = payment
        balance = round(balance - principal_part, 2)
        rows.append(
            {
                'month': month,
                'payment': payment_now,
                'interest': interest,
                'principal': principal_part,
                'balance': balance,
            }
        )
    return rows


def _expected() -> dict[str, Any]:
    npv: dict[str, float] = {}
    irr: dict[str, float] = {}
    for loan in _LOANS:
        payment = _payment(loan['principal'], loan['annual_rate'], loan['term_months'])
        npv[loan['id']] = round(_npv(loan['principal'], payment, loan['term_months']), 2)
        irr[loan['id']] = round(_irr(loan['principal'], payment, loan['term_months']), 4)
    loan = next(item for item in _LOANS if item['id'] == SCHEDULE_LOAN)
    rows = _schedule(loan['principal'], loan['annual_rate'], loan['term_months'])
    return {
        'npv': npv,
        'irr': irr,
        'schedule': {
            'payment': _payment(loan['principal'], loan['annual_rate'], loan['term_months']),
            'first': rows[0],
            'second': rows[1],
            'last': rows[-1],
            'total_interest': round(sum(float(row['interest']) for row in rows), 2),
        },
    }


REFERENCE = """
loans = await fetch_loans()

def payment_for(principal, annual_rate, months):
    r = annual_rate / 12
    return round(principal * r / (1 - (1 + r) ** -months), 2)

def npv_at(principal, payment, months, monthly):
    total = -principal
    for k in range(1, months + 1):
        total = total + payment / (1 + monthly) ** k
    return total

npv = {}
irr = {}
for loan in loans:
    payment = payment_for(loan['principal'], loan['annual_rate'], loan['term_months'])
    npv[loan['id']] = round(npv_at(loan['principal'], payment, loan['term_months'], DISCOUNT_RATE / 12), 2)
    lo = 0.0
    hi = 0.1
    for _ in range(100):
        mid = (lo + hi) / 2
        if npv_at(loan['principal'], payment, loan['term_months'], mid) > 0:
            lo = mid
        else:
            hi = mid
    irr[loan['id']] = round((lo + hi) / 2 * 12, 4)

loan = [item for item in loans if item['id'] == SCHEDULE_LOAN][0]
payment = payment_for(loan['principal'], loan['annual_rate'], loan['term_months'])
balance = loan['principal']
rows = []
total_interest = 0.0
for month in range(1, loan['term_months'] + 1):
    interest = round(balance * loan['annual_rate'] / 12, 2)
    if month == loan['term_months']:
        principal_part = round(balance, 2)
        payment_now = round(principal_part + interest, 2)
    else:
        principal_part = round(payment - interest, 2)
        payment_now = payment
    balance = round(balance - principal_part, 2)
    total_interest = total_interest + interest
    rows.append({'month': month, 'payment': payment_now, 'interest': interest, 'principal': principal_part, 'balance': balance})

{
    'npv': npv,
    'irr': irr,
    'schedule': {
        'payment': payment,
        'first': rows[0],
        'second': rows[1],
        'last': rows[-1],
        'total_interest': round(total_interest, 2),
    },
}
"""

TASK = Task(
    name='irr_ladder',
    category='numeric',
    prompt=(
        'For every loan, the level monthly payment is principal * r / (1 - (1 + r) ** -n) with '
        "r = annual_rate / 12 and n = term_months, rounded to cents. From the lender's side the "
        'cash flows are -principal at month 0 and that payment at months 1..n. Compute each '
        "loan's NPV at DISCOUNT_RATE (monthly rate DISCOUNT_RATE / 12, rounded to cents) and its "
        'IRR (find the monthly rate by bisection between 0 and 0.1 over 100 iterations, then '
        'multiply by 12, rounded to 4 decimal places). Then build the amortisation schedule for '
        'SCHEDULE_LOAN: each month interest = round(balance * annual_rate / 12, 2), principal = '
        'round(payment - interest, 2), balance = round(balance - principal, 2); in the final '
        'month the principal is the remaining balance and the payment is principal + interest. '
        'Return {"npv": {id: value}, "irr": {id: value}, "schedule": {"payment", "first", '
        '"second", "last", "total_interest"}} where first/second/last are the schedule rows '
        '(dicts with month, payment, interest, principal, balance) and total_interest is the '
        'sum of interest rounded to cents.'
    ),
    stubs=STUBS,
    tools={'fetch_loans': fetch_loans},
    inputs={'DISCOUNT_RATE': DISCOUNT_RATE, 'SCHEDULE_LOAN': SCHEDULE_LOAN},
    expected=_expected(),
    evaluators=(ApproxExpected(),),
    reference_solution=REFERENCE,
    traps=('decimal', 'numpy_financial reflex', 'float rounding drift over 360 rows'),
    expected_external_calls=1,
)
