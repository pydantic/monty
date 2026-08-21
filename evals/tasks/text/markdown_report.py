"""Render an aligned markdown table — formatting with only f-strings available.

`str.format()` and `%` formatting both raise in Monty, and column alignment is exactly
where a model reaches for `'{:<12}'.format(name)`. The f-string equivalent
(`f'{name:<12}'`) is right there, so this measures whether the prompt successfully
redirects the reflex rather than whether the task is possible.

Scored on the exact rendered string, because "roughly aligned" is not aligned: the
World Cup agent instructions in pydantic/talks had to shout about table whitespace in
capitals, which suggests models get this wrong often enough to be worth measuring.
"""

from __future__ import annotations

from typing import Any

from evals.harness.task import Exact, Task

_ROWS = [
    {'region': 'AMER', 'revenue': 3010.5, 'orders': 2},
    {'region': 'APAC', 'revenue': 2965.75, 'orders': 12},
    {'region': 'EMEA', 'revenue': 28300.0, 'orders': 143},
    {'region': 'LATAM', 'revenue': 1120.0, 'orders': 7},
]


async def fetch_regional_summary() -> list[dict[str, Any]]:
    """Host function: revenue and order count per region."""
    return [dict(row) for row in _ROWS]


STUBS = '''
from typing import Any

async def fetch_regional_summary() -> list[dict[str, Any]]:
    """Return revenue and order count per region.

    Each row has `region`, `revenue` (float) and `orders` (int).
    """
    ...
'''


def _expected() -> str:
    """Left-aligned region, right-aligned numbers, padded to the widest cell per column."""
    ordered = sorted(_ROWS, key=lambda row: float(row['revenue']), reverse=True)
    cells = [['Region', 'Revenue', 'Orders']]
    cells += [[str(row['region']), f'{float(row["revenue"]):,.2f}', str(row['orders'])] for row in ordered]
    widths = [max(len(row[column]) for row in cells) for column in range(3)]
    # Each dash run is exactly its column's width, with the alignment colon occupying
    # one of those characters — the same rule the prompt states.
    lines = [
        f'| {cells[0][0]:<{widths[0]}} | {cells[0][1]:>{widths[1]}} | {cells[0][2]:>{widths[2]}} |',
        f'| :{"-" * (widths[0] - 1)} | {"-" * (widths[1] - 1)}: | {"-" * (widths[2] - 1)}: |',
    ]
    for row in cells[1:]:
        lines.append(f'| {row[0]:<{widths[0]}} | {row[1]:>{widths[1]}} | {row[2]:>{widths[2]}} |')
    return '\n'.join(lines)


REFERENCE = """
rows = await fetch_regional_summary()
ordered = sorted(rows, key=lambda row: row['revenue'], reverse=True)

cells = [['Region', 'Revenue', 'Orders']]
for row in ordered:
    cells.append([row['region'], f'{row["revenue"]:,.2f}', str(row['orders'])])

widths = []
for column in range(3):
    widest = 0
    for row in cells:
        if len(row[column]) > widest:
            widest = len(row[column])
    widths.append(widest)

head = cells[0]
lines = [f'| {head[0]:<{widths[0]}} | {head[1]:>{widths[1]}} | {head[2]:>{widths[2]}} |']
lines.append(f'| :{"-" * (widths[0] - 1)} | {"-" * (widths[1] - 1)}: | {"-" * (widths[2] - 1)}: |')
for row in cells[1:]:
    lines.append(f'| {row[0]:<{widths[0]}} | {row[1]:>{widths[1]}} | {row[2]:>{widths[2]}} |')

'\\n'.join(lines)
"""

TASK = Task(
    name='markdown_report',
    category='text',
    prompt=(
        'Render the regional summary as a markdown table, highest revenue first. '
        'Columns: Region, Revenue, Orders. Format revenue with thousands separators and '
        'exactly 2 decimal places. Pad every cell so the columns line up: the Region '
        'column left-aligned, Revenue and Orders right-aligned, each padded to the width '
        'of the widest cell in that column (including the header). Use a separator row '
        'of the form `| :--- | ---: | ---: |` with each dash run matching its column '
        'width. Return the table as a single string with no trailing newline.'
    ),
    stubs=STUBS,
    tools={'fetch_regional_summary': fetch_regional_summary},
    expected=Exact(_expected()),
    reference_solution=REFERENCE,
    traps=('str.format', '% formatting', 'str.ljust/rjust', 'f-string nested width specs'),
    expected_external_calls=1,
)
