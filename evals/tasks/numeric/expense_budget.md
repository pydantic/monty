# expense_budget

For every member of the Engineering department, total their Q3 travel expenses and report who is over budget.
The standard budget is `STANDARD_BUDGET` (5000, passed as an input); `get_custom_budget` must only be called for people
who exceed it.

Host functions: `get_team_members(department)`, `get_expenses(user_id, quarter, category)` and
`get_custom_budget(user_id)`, ported from `examples/expense_analysis`.

Returns `total_team_members_analyzed`, `count_exceeded_budget` and `over_budget_details`, a list of
`name`/`total_spent`/`budget`/`amount_over` dicts.
Scored with `ApproxExpected`; nine host calls are expected (one roster, five expense lookups, three budget lookups).

The natural solution uses nothing Monty lacks, so this task is the control for the rest of the suite.
