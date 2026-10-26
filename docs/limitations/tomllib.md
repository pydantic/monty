# `tomllib` module

Monty's `tomllib` provides `loads`, `load` and the `TOMLDecodeError` exception.
The parser is a port of CPython's pure-Python implementation: the accepted grammar, the table and
namespace rules (dotted keys, redefinition, frozen inline containers) and the error messages, with
their `(at line N, column M)` / `(at end of document)` suffix, match CPython 3.14.

## `tomllib.loads(s, /)`

- `s` must be a `str`; anything else raises `TypeError: Expected str object, not '<type>'`,
    as in CPython.
- `parse_float` is not accepted: passing it raises `TypeError: loads() got an unexpected keyword argument 'parse_float'`.
    Floats are always `float`, so a document whose values need `decimal.Decimal` precision cannot be
    parsed losslessly.
- Passing `s` by keyword raises `loads() got an unexpected keyword argument 's'` where CPython reports
    `got some positional-only arguments passed as keyword arguments: 's'`.
- Inline arrays and inline tables may nest 100 levels deep; deeper input raises
    `RecursionError: maximum recursion depth exceeded`. CPython's limit depends on the interpreter
    recursion limit: at the default, nested arrays fail just under 500 levels and nested inline tables
    just over 330. Tables created by headers and dotted keys are
    not subject to this cap, only to CPython's 1000-part key limit (`RecursionError: TOML key has more than the allowed 1000 parts`),
    which Monty fixes at 1000 rather than reading `sys.getrecursionlimit()`.

## `tomllib.load(fp, /)`

- `fp` must be a file object returned by `open()` in a binary mode. Any other object raises
    `AttributeError: '<type>' object has no attribute 'read'`, even one that defines `read()`:
    the function cannot call back into Python.
- A text-mode file raises CPython's `TypeError` about binary mode before anything is read; CPython
    reads the text first, so a closed or undecodable text file fails differently there.
- `parse_float` is rejected as for `loads`.

## `TOMLDecodeError`

Inherits from `ValueError` (catchable as `except ValueError:`). The qualified name is
`tomllib.TOMLDecodeError`; `type(exc).__name__` matches CPython.

Inside the sandbox the exception is message-only: the `msg`, `doc`, `pos`, `lineno` and `colno`
attributes are not available, and the class cannot be instantiated with those arguments.
When a sandbox `TOMLDecodeError` surfaces to the host (e.g. via `pydantic_monty`), it is rebuilt as
a real `tomllib.TOMLDecodeError` with all five attributes from a structured payload attached at raise
time, except that documents larger than 64 KiB are not carried, in which case `doc` is `''`.
A `TOMLDecodeError` raised manually inside the sandbox has no payload and surfaces as a plain
`ValueError` carrying the message.
On a host Python without `tomllib` (3.10) the exception surfaces as its `ValueError` parent.
In the other direction, a `TOMLDecodeError` a host callback raises into the sandbox carries the five
attributes only on Python 3.14 or later; on 3.11 to 3.13 it enters without them, so if the sandbox
re-raises it the host receives a `ValueError`.

## Values

Parsed values use the same types as CPython: `str`, `int`, `float`, `bool`, `datetime.date`,
`datetime.time`, `datetime.datetime` (aware with a fixed-offset `timezone` when the document gives
an offset, `timezone.utc` for `Z`), `list` and `dict`. The differences are those of the underlying
types, see [datetime.md](datetime.md). Decimal integer literals longer than the `int()` string
conversion limit raise `ValueError` as in CPython (see [sys.md](sys.md)).
