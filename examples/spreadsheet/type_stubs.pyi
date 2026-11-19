"""What the sandbox can see of the document: the `openpyxl` API, less everything the host leaves out."""

from datetime import date, datetime, time, timedelta
from typing import Any, Literal, overload

CellValue = str | int | float | bool | datetime | date | time | timedelta | None

class Cell:
    """A snapshot of one cell; write with `Worksheet.cell(row, column, value)`."""

    coordinate: str
    row: int
    column: int
    column_letter: str
    value: CellValue
    data_type: str
    number_format: str
    is_date: bool

class Worksheet:
    title: str
    dimensions: str
    min_row: int
    max_row: int
    min_column: int
    max_column: int

    def cell(self, row: int, column: int, value: CellValue = None) -> Cell:
        """Returns the cell at `row`, `column` (both 1-based), setting its value if one is given."""

    def append(self, iterable: list[CellValue] | tuple[CellValue, ...] | dict[int | str, CellValue]) -> None:
        """Adds a row below the last one; a dict maps column letters or indexes to values."""

    @overload
    def iter_rows(
        self,
        min_row: int | None = None,
        max_row: int | None = None,
        min_col: int | None = None,
        max_col: int | None = None,
        values_only: Literal[False] = False,
    ) -> list[list[Cell]]: ...
    @overload
    def iter_rows(
        self,
        min_row: int | None = None,
        max_row: int | None = None,
        min_col: int | None = None,
        max_col: int | None = None,
        *,
        values_only: Literal[True],
    ) -> list[list[Any]]: ...
    @overload
    def iter_cols(
        self,
        min_col: int | None = None,
        max_col: int | None = None,
        min_row: int | None = None,
        max_row: int | None = None,
        values_only: Literal[False] = False,
    ) -> list[list[Cell]]: ...
    @overload
    def iter_cols(
        self,
        min_col: int | None = None,
        max_col: int | None = None,
        min_row: int | None = None,
        max_row: int | None = None,
        *,
        values_only: Literal[True],
    ) -> list[list[Any]]: ...

class Workbook:
    sheetnames: list[str]
    worksheets: list[Worksheet]
    active: Worksheet

    def create_sheet(self, title: str | None = None, index: int | None = None) -> Worksheet:
        """Adds a worksheet, at the end unless `index` is given."""

    def remove(self, worksheet: Worksheet) -> None:
        """Removes a worksheet from the document."""

    def save(self) -> None:
        """Saves the document in place; there is no other file to save to."""

wb: Workbook
