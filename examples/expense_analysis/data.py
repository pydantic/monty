from typing import Any

team_members = [
    {'id': 1, 'name': 'Alice Chen'},
    {'id': 2, 'name': 'Bob Smith'},
    {'id': 3, 'name': 'Carol Jones'},
    {'id': 4, 'name': 'David Kim'},
    {'id': 5, 'name': 'Eve Wilson'},
]

# Simulated expense data (multiple line items per person to bloat traditional context)
expenses = {
    1: [  # Alice - under budget
        {'date': '2024-07-15', 'amount': 450.00, 'description': 'Flight to NYC'},
        {'date': '2024-07-16', 'amount': 200.00, 'description': 'Hotel NYC'},
        {'date': '2024-07-17', 'amount': 85.00, 'description': 'Meals NYC'},
        {'date': '2024-08-20', 'amount': 380.00, 'description': 'Flight to Chicago'},
        {'date': '2024-08-21', 'amount': 175.00, 'description': 'Hotel Chicago'},
        {'date': '2024-09-05', 'amount': 520.00, 'description': 'Flight to Seattle'},
        {'date': '2024-09-06', 'amount': 225.00, 'description': 'Hotel Seattle'},
        {'date': '2024-09-07', 'amount': 95.00, 'description': 'Meals Seattle'},
    ],
    2: [  # Bob - over standard budget but has custom budget
        {'date': '2024-07-01', 'amount': 850.00, 'description': 'Flight to London'},
        {'date': '2024-07-02', 'amount': 450.00, 'description': 'Hotel London'},
        {'date': '2024-07-03', 'amount': 125.00, 'description': 'Meals London'},
        {'date': '2024-07-04', 'amount': 450.00, 'description': 'Hotel London'},
        {'date': '2024-07-05', 'amount': 120.00, 'description': 'Meals London'},
        {'date': '2024-08-10', 'amount': 780.00, 'description': 'Flight to Tokyo'},
        {'date': '2024-08-11', 'amount': 380.00, 'description': 'Hotel Tokyo'},
        {'date': '2024-08-12', 'amount': 380.00, 'description': 'Hotel Tokyo'},
        {'date': '2024-08-13', 'amount': 150.00, 'description': 'Meals Tokyo'},
        {'date': '2024-09-15', 'amount': 920.00, 'description': 'Flight to Singapore'},
        {'date': '2024-09-16', 'amount': 320.00, 'description': 'Hotel Singapore'},
        {'date': '2024-09-17', 'amount': 320.00, 'description': 'Hotel Singapore'},
        {'date': '2024-09-18', 'amount': 180.00, 'description': 'Meals Singapore'},
    ],
    3: [  # Carol - way over budget (no custom budget)
        {'date': '2024-07-08', 'amount': 1200.00, 'description': 'Flight to Paris'},
        {'date': '2024-07-09', 'amount': 550.00, 'description': 'Hotel Paris'},
        {'date': '2024-07-10', 'amount': 550.00, 'description': 'Hotel Paris'},
        {'date': '2024-07-11', 'amount': 550.00, 'description': 'Hotel Paris'},
        {'date': '2024-07-12', 'amount': 200.00, 'description': 'Meals Paris'},
        {'date': '2024-08-25', 'amount': 1100.00, 'description': 'Flight to Sydney'},
        {'date': '2024-08-26', 'amount': 480.00, 'description': 'Hotel Sydney'},
        {'date': '2024-08-27', 'amount': 480.00, 'description': 'Hotel Sydney'},
        {'date': '2024-08-28', 'amount': 480.00, 'description': 'Hotel Sydney'},
        {'date': '2024-08-29', 'amount': 220.00, 'description': 'Meals Sydney'},
        {'date': '2024-09-20', 'amount': 650.00, 'description': 'Flight to Denver'},
        {'date': '2024-09-21', 'amount': 280.00, 'description': 'Hotel Denver'},
    ],
    4: [  # David - slightly under budget
        {'date': '2024-07-22', 'amount': 420.00, 'description': 'Flight to Boston'},
        {'date': '2024-07-23', 'amount': 190.00, 'description': 'Hotel Boston'},
        {'date': '2024-07-24', 'amount': 75.00, 'description': 'Meals Boston'},
        {'date': '2024-08-05', 'amount': 510.00, 'description': 'Flight to Austin'},
        {'date': '2024-08-06', 'amount': 210.00, 'description': 'Hotel Austin'},
        {'date': '2024-08-07', 'amount': 90.00, 'description': 'Meals Austin'},
        {'date': '2024-09-12', 'amount': 480.00, 'description': 'Flight to Portland'},
        {'date': '2024-09-13', 'amount': 195.00, 'description': 'Hotel Portland'},
        {'date': '2024-09-14', 'amount': 85.00, 'description': 'Meals Portland'},
    ],
    5: [  # Eve - over standard budget (no custom budget)
        {'date': '2024-07-03', 'amount': 680.00, 'description': 'Flight to Miami'},
        {'date': '2024-07-04', 'amount': 320.00, 'description': 'Hotel Miami'},
        {'date': '2024-07-05', 'amount': 320.00, 'description': 'Hotel Miami'},
        {'date': '2024-07-06', 'amount': 145.00, 'description': 'Meals Miami'},
        {'date': '2024-08-18', 'amount': 750.00, 'description': 'Flight to San Diego'},
        {'date': '2024-08-19', 'amount': 290.00, 'description': 'Hotel San Diego'},
        {'date': '2024-08-20', 'amount': 290.00, 'description': 'Hotel San Diego'},
        {'date': '2024-08-21', 'amount': 130.00, 'description': 'Meals San Diego'},
        {'date': '2024-09-08', 'amount': 820.00, 'description': 'Flight to Las Vegas'},
        {'date': '2024-09-09', 'amount': 380.00, 'description': 'Hotel Las Vegas'},
        {'date': '2024-09-10', 'amount': 380.00, 'description': 'Hotel Las Vegas'},
        {'date': '2024-09-11', 'amount': 175.00, 'description': 'Meals Las Vegas'},
    ],
}

# Custom budgets (only Bob has one)
custom_budgets = {
    2: {'amount': 7000.00, 'reason': 'International travel required'},
}


async def get_team_members(department: str) -> dict[str, Any]:
    """Get list of team members for a department.

    Args:
        department: The department name (e.g., "Engineering").

    Returns:
        Dictionary with list of team members.
    """
    return {'department': department, 'members': team_members}


async def get_expenses(user_id: int, quarter: str, category: str) -> dict[str, Any]:
    """Get expense line items for a user.

    Args:
        user_id: The user's ID.
        quarter: The quarter (e.g., "Q3").
        category: The expense category (e.g., "travel").

    Returns:
        Dictionary with expense items.
    """
    items = expenses.get(user_id, [])
    return {'user_id': user_id, 'quarter': quarter, 'category': category, 'items': items}


async def get_custom_budget(user_id: int) -> dict[str, Any] | None:
    """Get custom budget for a user if they have one.

    Args:
        user_id: The user's ID.

    Returns:
        Custom budget info or None if no custom budget.
    """
    custom_budgets.get(user_id)
