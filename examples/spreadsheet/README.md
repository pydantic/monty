# Spreadsheet: an Excel file as dataclasses

Sandbox code reads an untidy Excel workbook through the [`openpyxl`](https://openpyxl.readthedocs.io/) objects
themselves, wrapped as [host objects](https://pydantic.dev/docs/monty/host-objects/), and returns it as a list of a
`Row` dataclass it defines.
It can reach the one document the host opened, and no other file.

```bash
uv run --group examples python examples/spreadsheet/main.py
```

`orders.xlsx` is an order sheet kept by hand: a title above the header, blank lines, a totals line, inconsistent
region names and numbers stored as text.
`main.py` runs `sandbox_code.py` in the sandbox, which defines `Row`, finds the table, cleans each line and constructs
a `Row` from it.
A line whose first cell is not a date raises `ValueError` naming the row.
`Row` belongs to the sandbox code, so the host receives each one as a read-only `MontyClassProxy` and prints its
`name` and `attributes`:

```text
Row {'date': datetime.date(2026, 7, 3), 'region': 'North', 'product': 'Widget', 'units': 12, 'unit_price': 9.5}
Row {'date': datetime.date(2026, 7, 8), 'region': 'East', 'product': 'Gadget', 'units': 2, 'unit_price': 42.0}
Row {'date': datetime.date(2026, 7, 19), 'region': 'South', 'product': 'Widget', 'units': 7, 'unit_price': 9.5}
...
```

## How the document is confined

`wrap_openpyxl.py` wraps the real `Workbook`, `Worksheet` and `Cell` in `ClassInstance` policies.
Every policy defaults to nothing, so the sandbox has only what is listed:

- `Workbook`
    - attributes: `sheetnames`, `worksheets`, `active`
    - methods: `create_sheet`, `remove`, `save`
- `Worksheet`
    - attributes: `title`, `dimensions`, `min_row`, `max_row`, `min_column`, `max_column`
    - methods: `cell`, `append`, `iter_rows`, `iter_cols`
- `Cell`
    - attributes: `coordinate`, `row`, `column`, `column_letter`, `value`, `data_type`, `number_format`, `is_date`
    - no methods

The rest of the confinement:

- `save()` takes no arguments and writes to the path the host loaded.
    `Workbook.save(filename)` is never called with a value from the sandbox.
- `openpyxl` is not importable in the sandbox, so there is no `load_workbook`, and `Worksheet.parent`, styles, images
    and charts are not exposed.
- `convert_value` wraps each worksheet and cell a call returns, and turns the generators `iter_rows` and `iter_cols`
    return into lists.
- `openpyxl` allocates a cell for every coordinate it is asked about, which the sandbox's `max_memory` does not count.
    `WorksheetWrapper.call_method` refuses row and column indexes beyond `MAX_ROWS` and `MAX_COLUMNS`, including the
    keys of a dict passed to `append`, and an `iter_rows` or `iter_cols` call whose bounds cover more than `MAX_CELLS`.
    `DocumentWrapper` refuses to grow the document past `MAX_SHEETS` worksheets.
    Each call suspends the sandbox, so `max_suspensions` bounds the number of calls.
- These limits are illustrative, and only bound what sandbox code asks for: the loaded file sets the sheet's own
    extent, which the default bounds of `iter_rows` and `iter_cols` follow.
    A production host must choose limits for its own workload.

`type_stubs.pyi` declares the same surface for the type checker, so code that calls `wb.save('other.xlsx')` fails
before it runs.

## Differences from `openpyxl`

- Subscripting is not dispatched to host objects: use `ws.cell(row, column)` for `ws['A1']`, and `wb.worksheets` or
    `wb.sheetnames` for `wb['Orders']`.
- A cell is a snapshot taken when it crossed, and a merged cell has only `coordinate`, `row`, `column` and `value`.
    Write with `ws.cell(row, column, value)` or `ws.append(...)`; assigning `cell.value` changes the sandbox's copy only.
- Worksheet attributes are lazy, so `ws.max_row` reflects rows the sandbox has appended.
- Prefer `iter_rows(values_only=True)`: without it every cell crosses as its own host object, which the session keeps
    until it ends.
- Formulas are stored as strings and never evaluated.
