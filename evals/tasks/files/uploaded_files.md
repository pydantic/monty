# uploaded_files

Count the data rows of every CSV in `/data`, a host directory mounted read-only, report the total and the number of
quoted fields, and show that a write there is refused.
`setup` writes three CSVs (two with quoted fields containing commas) and one `README.txt` that must be skipped.

No host functions.
The code lists the directory with `Path.iterdir()` and parses the CSVs by hand (`csv` would be the natural tool;
Monty has none, and no `Path.glob`).

Scored with `EqualsExpected`.
The runtime raises `PermissionError` on the write, but the bundled type stubs do not define that name, so
`except PermissionError` fails the type check before the code runs; the reference catches `OSError`.
