"""Apply a written expense policy plus three user-submitted plugin rules to a month of claims.

The policy is prose in the prompt and the plugins are Python source the model must
paste into its script unchanged, so the rules run as functions defined in the
sandbox. One plugin raises on some claims; those errors are reported per claim and
the claim is still decided by the remaining rules. The expected decisions are
computed host-side by the very same functions, so there is one source of truth.

Monty functions have no `__name__`, so the reference pairs each plugin with its name
by hand; `plugin.__name__` is the natural spelling and raises `AttributeError`.
"""

from __future__ import annotations

import inspect
import random
from collections.abc import Callable
from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.task import Task

POLICY = """\
1. Meals are reimbursed up to 50.00 per claim.
2. Hotel nights are reimbursed up to 200.00 per claim.
3. Any claim over 25.00 needs a receipt.
4. Entertainment claims need a manager's approval: the notes must contain "approved by".
5. Claims dated on a weekend are rejected unless the category is "travel".
"""


def rule_weekend(claim: dict[str, Any]) -> str | None:
    """Reject non-travel spend on Saturdays and Sundays."""
    from datetime import date

    if date.fromisoformat(claim['date']).weekday() >= 5 and claim['category'] != 'travel':
        return 'weekend claim outside travel'
    return None


def rule_round_amount(claim: dict[str, Any]) -> str | None:
    """Flag suspiciously round amounts of 100 or more."""
    amount = float(claim['amount'])
    if amount >= 100 and amount == int(amount) and int(amount) % 50 == 0:
        return f'round amount {amount:.2f} needs a second look'
    return None


def rule_vendor(claim: dict[str, Any]) -> str | None:
    """Require a vendor in the notes, e.g. `vendor: Acme`; raises when notes are missing."""
    notes = claim['notes']
    if notes is None:
        raise ValueError('claim has no notes')
    if 'vendor:' not in str(notes):
        return 'no vendor recorded'
    return None


PLUGINS: tuple[Callable[[dict[str, Any]], str | None], ...] = (rule_weekend, rule_round_amount, rule_vendor)
PLUGIN_SOURCE = 'from typing import Any\n\n\n' + '\n\n'.join(inspect.getsource(fn) for fn in PLUGINS)

_CATEGORIES = ('meals', 'hotel', 'travel', 'entertainment', 'supplies')
_EMPLOYEES = ('priya', 'tomas', 'wei', 'lena', 'omar')


def _build_claims() -> list[dict[str, Any]]:
    rng = random.Random(2026)
    claims: list[dict[str, Any]] = []
    for index in range(40):
        category = rng.choice(_CATEGORIES)
        base = {'meals': 40, 'hotel': 180, 'travel': 120, 'entertainment': 90, 'supplies': 30}[category]
        amount = round(base * rng.uniform(0.5, 1.6), 2)
        if rng.random() < 0.15:
            amount = float(rng.choice((100, 150, 200, 250)))
        day = rng.randrange(1, 31)
        notes: str | None = rng.choice(('vendor: Acme', 'vendor: Globex', 'lunch with client', None))
        if category == 'entertainment' and rng.random() < 0.5:
            notes = 'approved by M. Hamilton, vendor: Initech'
        claims.append(
            {
                'id': f'C-{index + 1:03d}',
                'employee': rng.choice(_EMPLOYEES),
                'category': category,
                'amount': amount,
                'date': f'2026-08-{day:02d}',
                'receipt': rng.random() < 0.75,
                'notes': notes,
            }
        )
    return claims


CLAIMS = _build_claims()


def _policy_reasons(claim: dict[str, Any]) -> list[str]:
    reasons: list[str] = []
    if claim['category'] == 'meals' and claim['amount'] > 50:
        reasons.append('meal over 50.00')
    if claim['category'] == 'hotel' and claim['amount'] > 200:
        reasons.append('hotel night over 200.00')
    if claim['amount'] > 25 and not claim['receipt']:
        reasons.append('no receipt')
    if claim['category'] == 'entertainment' and 'approved by' not in (claim['notes'] or ''):
        reasons.append('entertainment without approval')
    return reasons


def _decide(claim: dict[str, Any]) -> dict[str, Any]:
    reasons = _policy_reasons(claim)
    errors: list[str] = []
    for plugin in PLUGINS:
        try:
            reason = plugin(claim)
        except ValueError as exc:
            errors.append(f'{plugin.__name__}: {exc}')
            continue
        if reason is not None:
            reasons.append(f'{plugin.__name__}: {reason}')
    return {'id': claim['id'], 'decision': 'rejected' if reasons else 'approved', 'reasons': reasons, 'errors': errors}


EXPECTED = [_decide(c) for c in CLAIMS]

STUBS = '''
from typing import Any

CLAIMS: list[dict[str, Any]] = []
"""Each claim has `id`, `employee`, `category`, `amount` (float), `date` (ISO), `receipt` (bool) and `notes` (str or None)."""
'''

REFERENCE = f"""
{PLUGIN_SOURCE}

plugins = [('rule_weekend', rule_weekend), ('rule_round_amount', rule_round_amount), ('rule_vendor', rule_vendor)]

def policy_reasons(claim):
    reasons = []
    if claim['category'] == 'meals' and claim['amount'] > 50:
        reasons.append('meal over 50.00')
    if claim['category'] == 'hotel' and claim['amount'] > 200:
        reasons.append('hotel night over 200.00')
    if claim['amount'] > 25 and not claim['receipt']:
        reasons.append('no receipt')
    notes = claim['notes'] if claim['notes'] is not None else ''
    if claim['category'] == 'entertainment' and 'approved by' not in notes:
        reasons.append('entertainment without approval')
    return reasons

decisions = []
for claim in CLAIMS:
    reasons = policy_reasons(claim)
    errors = []
    for plugin_name, plugin in plugins:
        try:
            reason = plugin(claim)
        except ValueError as exc:
            errors.append(f'{{plugin_name}}: {{exc}}')
            continue
        if reason is not None:
            reasons.append(f'{{plugin_name}}: {{reason}}')
    decisions.append({{
        'id': claim['id'],
        'decision': 'rejected' if reasons else 'approved',
        'reasons': reasons,
        'errors': errors,
    }})

decisions
"""

TASK = Task(
    name='expense_policy',
    category='business',
    prompt=(
        'Decide every claim in CLAIMS against the expense policy below and the three plugin rules. '
        'Include the plugin source in your code exactly as given and call each plugin on each claim. '
        'A plugin returns a reason string to reject or None to pass; if it raises ValueError, record '
        'the error as "<function name>: <message>" for that claim and carry on with the other rules. '
        'Policy reasons are, in this order and with these exact texts: "meal over 50.00", '
        '"hotel night over 200.00", "no receipt", "entertainment without approval". Plugin reasons '
        'are "<function name>: <returned reason>" in plugin order after the policy reasons. A claim '
        'with no reasons is "approved", otherwise "rejected". Return a list, in CLAIMS order, of dicts '
        'with "id", "decision", "reasons" and "errors".\n\n'
        f'Policy:\n{POLICY}\nPlugins:\n```python\n{PLUGIN_SOURCE}```'
    ),
    stubs=STUBS,
    tools={},
    inputs={'CLAIMS': CLAIMS},
    expected=EXPECTED,
    evaluators=(EqualsExpected(),),
    reference_solution=REFERENCE,
    traps=('function.__name__', 'local import inside a function', 'exceptions caught per record'),
)
