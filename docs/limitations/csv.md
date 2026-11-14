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
    `csv.Dialect` is `_csv.Dialect`, the resolved-dialect type, rather than CPython's Python base class:
    it takes a dialect and the format parameters (`csv.Dialect(delimiter=';')`), and `csv.Dialect()`
    is the `excel` dialect where CPython raises for its unset `delimiter`.
- **`reader(f, 'excel', dialect='unix')` numbers the clashing position differently.**
    The message is `argument for function given by name ('dialect') and position (2)`; CPython
    forwards the keyword to `Dialect()` and reports position 1.
- **`reader.dialect` and `writer.dialect` are fresh objects.**
    Each access builds a new `_csv.Dialect` holding the resolved parameters, so `r.dialect is r.dialect` is
    `False`, where CPython returns the object stored at construction.
- **A file is loaded when the reader is built.**
    `csv.reader(f)` and `csv.DictReader(f)` over an `open()` file nobody has read yet read the whole file
    at construction so that any consumer (`list(reader)`, `next(reader)`, a `for` loop) works;
    CPython reads lazily on the first `__next__`.
    See [open.md](open.md) for the file buffer this relies on.
- **The file is loaded before the arguments are checked.**
    When the reader's file is loaded at construction, a host read error is reported before a bad
    dialect, format parameter or keyword would be; CPython validates the arguments first and never reads
    there.
- **`writer.writerows()` on a file issues one write.**
    The rows are formatted and written to the file in a single host call, where CPython calls `write()`
    once per row, so a row that fails to format leaves the file untouched instead of partly written.
    `DictWriter.writerows()` on a file flattens every dict before writing for the same reason.
    Any other target (an object with a `write()` method) is called once per row as the rows arrive, but
    synchronously: a `write()` that reaches an external function or host call raises `NotImplementedError`
    (see [classes.md](classes.md) for this restriction).
    `writerow()` has no such limit, since it returns whatever the single `write()` call returns.
- **`writerow()` reads the whole row before formatting any field.**
    CPython formats each field as the row yields it, so with a row iterator that has side effects or
    raises part-way, the order of a field's `str()` call and the iterator's own error can differ.
- **`DictWriter` rows must be `dict`s.**
    `rowdict.keys()` and `rowdict.get()` are only resolved for real dicts; another mapping type raises
    the same `AttributeError` CPython raises for a non-mapping.
- **`extrasaction='raise'` reports unknown keys in dict order.**
    CPython builds the message from a set difference, so its order varies.
- **`DictReader.reader`, `DictReader.line_num` and `DictWriter.writer` are read-only.**
    Assigning them raises `AttributeError` where CPython, whose `DictReader` and `DictWriter` are plain
    Python classes, allows it.
    `fieldnames`, `restkey`, `restval` and `dialect` on a `DictReader` and `fieldnames`, `restval` and
    `extrasaction` on a `DictWriter` can be assigned.
- **`copy.copy()` and `copy.deepcopy()` of a `DictReader` or `DictWriter` raise `TypeError`.**
    The message is `cannot pickle 'csv.DictReader' object`; CPython copies the wrapper, sharing the
    underlying `_csv.reader` or `_csv.writer` on a shallow copy.
- **`repr(csv.Error('x'))` is `_csv.Error('x')`**, where CPython gives `Error('x')`, as for the other
    dotted exception classes (see [exceptions.md](exceptions.md)).
- **A dialect object's attributes cannot come from properties.**
    `dialect=SomeClass` reads plain attributes (`delimiter = ';'`); an attribute implemented as a `property`
    raises `NotImplementedError`.
