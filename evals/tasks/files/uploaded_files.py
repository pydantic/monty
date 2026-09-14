"""Analyse a directory of CSV files the user mounted read-only.

`setup` writes four files into a host directory that is mounted at `/data` in
read-only mode. The code has to discover them with `pathlib`, parse the CSVs itself
(`csv` would be the natural tool; Monty has none) including one file with quoted
fields containing commas, skip the file that is not a CSV, and prove the mount is
read-only by catching the error a write raises. The runtime raises `PermissionError`,
but the bundled type stubs do not define that name, so the reference catches
`OSError`; `except PermissionError` fails the type check before the code runs.
"""

from __future__ import annotations

from pathlib import Path

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.task import Task
from pydantic_monty import MountDir

DATA_DIR = Path(__file__).parent.parent.parent / 'reports' / 'artifacts' / 'uploaded_files'
DATA_DIR.mkdir(parents=True, exist_ok=True)

_FILES = {
    'customers.csv': 'id,name,company\n1,Ada Lovelace,"Analytical Engines, Ltd"\n2,Grace Hopper,Compiler Works\n3,Alan Turing,"Bletchley, Park & Co"\n',
    'orders.csv': 'order_id,customer_id,amount\n'
    + ''.join(f'o-{i},{i % 3 + 1},{10.5 * i:.2f}\n' for i in range(1, 26)),
    'products.csv': 'sku,name,price\nW-1,widget,9.99\nG-1,gadget,19.99\nD-1,"doohickey, large",4.50\nT-1,thingamajig,2.25\n',
    'README.txt': 'Exported 2026-09-01. Not a CSV.\n',
}


def _reset() -> None:
    """Write the fixture files fresh, so a previous attempt cannot have changed them."""
    for name, content in _FILES.items():
        (DATA_DIR / name).write_text(content)
    for stray in DATA_DIR.iterdir():
        if stray.name not in _FILES:
            stray.unlink()


EXPECTED = {
    'files': {name: content.count('\n') - 1 for name, content in _FILES.items() if name.endswith('.csv')},
    'total': sum(content.count('\n') - 1 for name, content in _FILES.items() if name.endswith('.csv')),
    'quoted_fields': 3,
    'write_blocked': True,
}

STUBS = ''

REFERENCE = """
from pathlib import Path

def split_csv_line(line):
    fields = []
    current = ''
    in_quotes = False
    quoted = False
    for char in line:
        if char == '"':
            in_quotes = not in_quotes
            quoted = True
        elif char == ',' and not in_quotes:
            fields.append((current, quoted))
            current = ''
            quoted = False
        else:
            current = current + char
    fields.append((current, quoted))
    return fields

files = {}
quoted_fields = 0
for path in sorted(Path('/data').iterdir(), key=lambda p: p.name):
    if not path.name.endswith('.csv'):
        continue
    lines = [line for line in path.read_text().split('\\n') if line.strip()]
    files[path.name] = len(lines) - 1
    for line in lines[1:]:
        for _, quoted in split_csv_line(line):
            if quoted:
                quoted_fields += 1

try:
    Path('/data/scratch.txt').write_text('test')
    write_blocked = False
except OSError:
    write_blocked = True

{'files': files, 'total': sum(files.values()), 'quoted_fields': quoted_fields, 'write_blocked': write_blocked}
"""

TASK = Task(
    name='uploaded_files',
    category='files',
    prompt=(
        'The user has uploaded files to the directory /data. For every CSV file in it (by extension), '
        'count its data rows, excluding the header; fields may be quoted with double quotes and a '
        'quoted field may contain commas. Return {"files": {<file name>: <rows>}, "total": <rows across '
        'all CSVs>, "quoted_fields": <number of quoted data fields across all CSVs>, "write_blocked": '
        '<True if an attempt to create /data/scratch.txt raised PermissionError, else False>}.'
    ),
    stubs=STUBS,
    tools={},
    mounts=[MountDir(host_path=DATA_DIR, virtual_path='/data', mode='read-only')],
    expected=EXPECTED,
    evaluators=(EqualsExpected(),),
    reference_solution=REFERENCE,
    traps=('csv module', 'Path.glob', 'quoted CSV fields', 'PermissionError missing from the type stubs'),
    expected_external_calls=0,
    setup=_reset,
)
