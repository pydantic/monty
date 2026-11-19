"""Sandboxed code that runs inside Monty: reads the order sheet and returns it as `Row`s.

`wb` is the document, passed in by the host as an input and declared in `type_stubs.pyi`.
The last expression is the value returned to the host.
"""

import datetime
from dataclasses import dataclass
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_stubs import wb


@dataclass
class Row:
    date: datetime.date
    region: str
    product: str
    units: int
    unit_price: float


sheet = wb.active
lines = sheet.iter_rows(values_only=True)

# the table starts below a title and a blank line
header = [line[0] for line in lines].index('Date')

rows: list[Row] = []
for number, line in enumerate(lines[header + 1 :], start=header + 2):
    date, region, product, units, unit_price = line
    if date == 'Total':
        break
    elif all(value is None for value in line):
        continue
    elif not isinstance(date, datetime.datetime):
        raise ValueError(f'row {number}: expected a date in column A, got {date!r}')
    elif any(value is None for value in line):
        raise ValueError(f'row {number}: missing values')

    row = Row(
        date=date.date(),
        region=region.strip().title(),
        product=product,
        units=int(units),
        unit_price=float(unit_price),
    )
    rows.append(row)

rows  # pyright: ignore[reportUnusedExpression]
