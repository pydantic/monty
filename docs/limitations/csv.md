# `csv` module

Monty's `csv` module is a Rust port of CPython's `_csv` parser and writer plus the `DictReader` and
`DictWriter` wrappers.
Field splitting, quoting, escaping, the `QUOTE_*` modes and the error messages match CPython 3.14 apart
from the notes below.

## Implemented

`reader`, `writer`, `DictReader`, `DictWriter`, `Error`, the six `QUOTE_*` constants, `get_dialect`,
`list_dialects`, `field_size_limit` (read-only, see below) and the three built-in dialects `excel`,
`excel_tab` and `unix_dialect`.

## Not implemented

- `register_dialect`, `unregister_dialect` and `Sniffer` are missing from the module namespace.
    Module attributes cannot hold state between imports (see [modules.md](modules.md)), so there is no
    dialect registry; pass format parameters or a dialect object instead.
- `csv.field_size_limit(new_limit)` raises `NotImplementedError: field_size_limit(): changing the limit is not supported`.
    The limit is fixed at CPython's default, 131072 characters, and `field_size_limit()` reports it.
- `__version__` is missing.

## Behavioural divergences

- **The built-in dialects are `_csv.Dialect` objects, not classes.**
    `csv.excel.delimiter` and `dialect=csv.excel` work, but `csv.excel()` and subclassing it do not.
    `csv.Dialect` is `_csv.Dialect`, the resolved-dialect type, rather than CPython's Python base class.
- **`reader.dialect` and `writer.dialect` are fresh objects.**
    Each access builds a new `_csv.Dialect` holding the resolved parameters, so `r.dialect is r.dialect` is
    `False`, where CPython returns the object stored at construction.
- **A file is loaded when the reader is built.**
    `csv.reader(f)` and `csv.DictReader(f)` over an `open()` file nobody has read yet read the whole file
    at construction so that any consumer (`list(reader)`, `next(reader)`, a `for` loop) works;
    CPython reads lazily on the first `__next__`.
    See [open.md](open.md) for the file buffer this relies on.
- **`writer.writerows()` on a file issues one write.**
    The rows are formatted and written to the file in a single host call, where CPython calls `write()`
    once per row, so a row that fails to format leaves the file untouched instead of partly written.
    `DictWriter.writerows()` flattens every dict before writing for the same reason.
    Any other target (an object with a `write()` method) is called once per row, but synchronously: a
    `write()` that reaches an external function or host call raises `NotImplementedError`
    (see [classes.md](classes.md) for this restriction).
    `writerow()` has no such limit, since it returns whatever the single `write()` call returns.
- **`DictWriter` rows must be `dict`s.**
    `rowdict.keys()` and `rowdict.get()` are only resolved for real dicts; another mapping type raises
    the same `AttributeError` CPython raises for a non-mapping.
- **`extrasaction='raise'` reports unknown keys in dict order.**
    CPython builds the message from a set difference, so its order varies.
- **`DictReader` attributes other than `fieldnames`, `restkey` and `restval` are read-only.**
    `reader.line_num = 0` or `reader.dialect = 'unix'` raise `AttributeError` where CPython allows the
    assignment.
    `DictWriter` attributes are all read-only.
- **A dialect object's attributes cannot come from properties.**
    `dialect=SomeClass` reads plain attributes (`delimiter = ';'`); an attribute implemented as a `property`
    raises `NotImplementedError`.
