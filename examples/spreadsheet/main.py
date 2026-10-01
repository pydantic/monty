"""Spreadsheet example: sandbox code reads one `openpyxl` workbook and returns it as dataclasses."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from wrap_openpyxl import open_document

from pydantic_monty import Monty, MontyClassProxy

THIS_DIR = Path(__file__).parent
TYPE_STUBS = (THIS_DIR / 'type_stubs.pyi').read_text()
SANDBOX_CODE = (THIS_DIR / 'sandbox_code.py').read_text()


def main() -> None:
    with Monty() as pool:
        with pool.checkout(
            script_name='spreadsheet.py',
            type_check=True,
            type_check_stubs=TYPE_STUBS,
        ) as session:
            rows: list[Any] = session.feed_run(
                SANDBOX_CODE,
                inputs={'wb': open_document(THIS_DIR / 'orders.xlsx')},
            )

    # `Row` is defined by the sandbox code, so each one arrives as a read-only proxy
    for row in rows:
        assert isinstance(row, MontyClassProxy)
        print(row.name, row.attributes)


if __name__ == '__main__':
    main()
