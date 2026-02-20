import data

import pydantic_monty

type_definitions = '''
from typing import Any

async def get_team_members(department: str) -> dict[str, Any]:
    """Get list of team members for a department.
    Args:
        department: The department name (e.g., "Engineering").
    Returns:
        Dictionary with list of team members.
    """
    ...

async def get_expenses(user_id: int, quarter: str, category: str) -> dict[str, Any]:
    """Get expense line items for a user.
    Args:
        user_id: The user's ID.
        quarter: The quarter (e.g., "Q3").
        category: The expense category (e.g., "travel").
    Returns:
        Dictionary with expense items.
    """
    ...

async def get_custom_budget(user_id: int) -> dict[str, Any] | None:
    """Get custom budget for a user if they have one.
    Args:
        user_id: The user's ID.
    Returns:
        Custom budget info or None if no custom budget.
    """
    ...
'''

code = """
# Get Engineering team members
team_data = await get_team_members(department="Engineering")
team_members = team_data.get("members", [])

# Standard budget
STANDARD_BUDGET = 5000

# Process each team member
total_members = len(team_members)
over_budget_list = []

for member in team_members:
    user_id = member.get("id")
    name = member.get("name")

    # Get Q3 travel expenses for this user
    expenses_data = await get_expenses(user_id=user_id, quarter="Q3", category="travel")
    expense_items = expenses_data.get("expenses", [])

    # Sum up total expenses
    total_spent = sum(item.get("amount", 0) for item in expense_items)

    # Check if they exceeded standard budget
    if total_spent > STANDARD_BUDGET:
        # Check for custom budget
        custom_budget_data = await get_custom_budget(user_id=user_id)

        if custom_budget_data is not None:
            budget = custom_budget_data.get("budget", STANDARD_BUDGET)
        else:
            budget = STANDARD_BUDGET

        # Check if they exceeded their actual budget (standard or custom)
        if total_spent > budget:
            amount_over = total_spent - budget
            over_budget_list.append({
                "name": name,
                "total_spent": total_spent,
                "budget": budget,
                "amount_over": amount_over
            })

# Return the analysis
{
    "total_team_members_analyzed": total_members,
    "count_exceeded_budget": len(over_budget_list),
    "over_budget_details": over_budget_list
}
"""


m = pydantic_monty.Monty(
    code,
    inputs=['prompt'],
    external_functions=['get_team_members', 'get_expenses', 'get_custom_budget'],
    script_name='expense.py',
    type_check=True,
    type_check_stubs=type_definitions,
)


async def main():
    output = await pydantic_monty.run_monty_async(
        m,
        inputs={'prompt': 'testing'},
        external_functions={
            'get_team_members': data.get_team_members,
            'get_expenses': data.get_expenses,
            'get_custom_budget': data.get_custom_budget,
        },
    )
    print(output)


if __name__ == '__main__':
    import asyncio

    asyncio.run(main())
