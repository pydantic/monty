"""Budget analysis with a conditional second lookup — the control task.

Ported from `examples/expense_analysis`, which exists because Anthropic uses the same
scenario to motivate programmatic tool calling. Nothing here is a trap: every construct
the obvious solution needs, Monty has.

That is the point. It anchors the scoreboard: if a prompt variant fails *this*, the
problem is the prompt itself rather than any missing Monty feature, and a regression
here means the harness broke rather than the model.
"""

from __future__ import annotations

from typing import Any

from evals.harness.task import Approx, Task

STANDARD_BUDGET = 5000

_TEAM = [
    {'id': 1, 'name': 'Priya Raman'},
    {'id': 2, 'name': 'Tomas Nowak'},
    {'id': 3, 'name': 'Wei Chen'},
    {'id': 4, 'name': 'Lena Fischer'},
    {'id': 5, 'name': 'Omar Haddad'},
]

_EXPENSES: dict[int, list[float]] = {
    1: [1200.0, 800.0, 450.0],
    2: [3000.0, 2600.0, 900.0],
    3: [200.0],
    4: [4000.0, 2500.0],
    5: [2000.0, 1800.0, 1400.0],
}

_CUSTOM_BUDGETS = {2: 7000, 4: 6000}


async def get_team_members(department: str) -> dict[str, Any]:
    """Host function: the members of a department."""
    if department != 'Engineering':
        return {'members': []}
    return {'members': [dict(member) for member in _TEAM]}


async def get_expenses(user_id: int, quarter: str, category: str) -> dict[str, Any]:
    """Host function: one user's expense line items for a quarter and category."""
    amounts = _EXPENSES.get(user_id, [])
    return {'expenses': [{'amount': amount, 'category': category, 'quarter': quarter} for amount in amounts]}


async def get_custom_budget(user_id: int) -> dict[str, Any] | None:
    """Host function: a user's custom budget, or `None` when they use the standard one."""
    if user_id not in _CUSTOM_BUDGETS:
        return None
    return {'user_id': user_id, 'budget': _CUSTOM_BUDGETS[user_id]}


STUBS = '''
from typing import Any

STANDARD_BUDGET: int = 5000
"""The default travel budget per person per quarter."""

async def get_team_members(department: str) -> dict[str, Any]:
    """Get the members of a department.

    Returns `{"members": [{"id": int, "name": str}, ...]}`.
    """
    ...

async def get_expenses(user_id: int, quarter: str, category: str) -> dict[str, Any]:
    """Get one user's expense line items.

    Returns `{"expenses": [{"amount": float, ...}, ...]}`.
    """
    ...

async def get_custom_budget(user_id: int) -> dict[str, Any] | None:
    """Get a user's custom budget, or `None` if they have none.

    Returns `{"user_id": int, "budget": int}` when one exists.
    """
    ...
'''


def _expected() -> dict[str, Any]:
    over: list[dict[str, Any]] = []
    for member in _TEAM:
        user_id = int(member['id'])
        spent = sum(_EXPENSES.get(user_id, []))
        if spent <= STANDARD_BUDGET:
            continue
        budget = _CUSTOM_BUDGETS.get(user_id, STANDARD_BUDGET)
        if spent > budget:
            over.append(
                {
                    'name': member['name'],
                    'total_spent': spent,
                    'budget': budget,
                    'amount_over': round(spent - budget, 2),
                }
            )
    return {
        'total_team_members_analyzed': len(_TEAM),
        'count_exceeded_budget': len(over),
        'over_budget_details': over,
    }


REFERENCE = """
team = await get_team_members(department='Engineering')
members = team['members']

over_budget = []
for member in members:
    expenses = await get_expenses(user_id=member['id'], quarter='Q3', category='travel')
    total_spent = sum(item['amount'] for item in expenses['expenses'])
    if total_spent > STANDARD_BUDGET:
        custom = await get_custom_budget(user_id=member['id'])
        budget = custom['budget'] if custom is not None else STANDARD_BUDGET
        if total_spent > budget:
            over_budget.append({
                'name': member['name'],
                'total_spent': total_spent,
                'budget': budget,
                'amount_over': round(total_spent - budget, 2),
            })

{
    'total_team_members_analyzed': len(members),
    'count_exceeded_budget': len(over_budget),
    'over_budget_details': over_budget,
}
"""

TASK = Task(
    name='expense_budget',
    category='numeric',
    prompt=(
        'For every member of the Engineering department, total their Q3 travel expenses. '
        f'The standard budget is {STANDARD_BUDGET}, but some people have a custom budget — '
        'only look that up for people who exceed the standard one. Return a dict with '
        '"total_team_members_analyzed", "count_exceeded_budget", and "over_budget_details": '
        'a list of dicts with "name", "total_spent", "budget" and "amount_over" for each '
        'person over their actual budget.'
    ),
    stubs=STUBS,
    tools={
        'get_team_members': get_team_members,
        'get_expenses': get_expenses,
        'get_custom_budget': get_custom_budget,
    },
    inputs={'STANDARD_BUDGET': STANDARD_BUDGET},
    expected=Approx(_expected()),
    reference_solution=REFERENCE,
    traps=(),
    # 1 roster + 5 expense lookups + a budget lookup for each of the 3 over the standard.
    expected_external_calls=1 + len(_TEAM) + 3,
)
