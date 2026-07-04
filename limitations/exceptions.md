# Exceptions

Monty implements a fixed set of exception classes, listed below. Sandboxed
code **cannot define new exception classes** (no `class` statement; see
[language.md](language.md)) — `raise` must use one of these built-ins.

## Implemented exception classes

`BaseException`, `Exception`, `SystemExit`, `KeyboardInterrupt`,
`ArithmeticError`, `OverflowError`, `ZeroDivisionError`, `LookupError`,
`IndexError`, `KeyError`, `RuntimeError`, `NotImplementedError`,
`RecursionError`, `AttributeError`, `FrozenInstanceError`, `NameError`,
`UnboundLocalError`, `ValueError`, `UnicodeDecodeError`, `UnicodeEncodeError`,
`ImportError`, `ModuleNotFoundError`, `OSError`, `FileNotFoundError`, `FileExistsError`,
`IsADirectoryError`, `NotADirectoryError`, `PermissionError`,
`AssertionError`, `MemoryError`, `StopIteration`, `SyntaxError`,
`TimeoutError`, `TypeError`.

Module-specific: `json.JSONDecodeError` (subclass of `ValueError`),
`re.PatternError` / `re.error`, `io.UnsupportedOperation` (catchable as
both `OSError` and `ValueError`, matching CPython's dual parentage).

## Exception classes NOT implemented

`Warning` and all its subclasses (`DeprecationWarning`, etc.),
`BufferError`, `EOFError`, `FloatingPointError`, `GeneratorExit`,
`ConnectionError` and subclasses (`ConnectionAbortedError`,
`ConnectionRefusedError`, `ConnectionResetError`,
`BrokenPipeError`), `BlockingIOError`, `ChildProcessError`,
`InterruptedError`, `ProcessLookupError`, `ReferenceError`,
`StopAsyncIteration`, `SystemError`, `TabError`, `IndentationError`,
`UnicodeError` (parent), `UnicodeTranslateError`,
`EncodingWarning`, `EnvironmentError` / `IOError` aliases,
`ExceptionGroup` / `BaseExceptionGroup` (see [language.md](language.md)).

## Constructor signature

All exception constructors accept **zero or one string argument** only.
Multi-argument forms used in CPython (e.g. `OSError(errno, strerror,
filename)`, `UnicodeDecodeError(encoding, obj, start, end, reason)`) are
not supported — passing more than one argument raises an internal error.

## Attributes

- `exc.args` — a tuple with 0 or 1 elements. Always a `tuple`, even when
  empty.
- `str(exc)` — returns the single message string, or `""` if none.
- `repr(exc)` — `ClassName('message')` matching CPython, **except**
  `UnicodeDecodeError`/`UnicodeEncodeError`: CPython reprs these from their
  real 5-field constructor (`UnicodeDecodeError('ascii', b'\xff', 0, 1,
  'ordinal not in range(128)')`), which Monty doesn't track — Monty's
  `repr()` uses the generic single-message form instead.

**Not implemented:** `__cause__`, `__context__`, `__suppress_context__`,
`__traceback__`, `__notes__`, `add_note()`. The `raise X from Y` syntax
parses, but the `from Y` cause is **silently dropped** — chained
tracebacks are not preserved across `raise from`.

## Custom subclasses

Because user `class` definitions are rejected at parse time, there is no
way to create a new exception class inside the sandbox. Define custom
exception types on the host side if needed, or use the built-in subclass
that best fits.

## Traceback behaviour

Tracebacks are formatted to match CPython, including the
`File "...", line N, in <function>` lines and `~` caret markers (Monty
uses `~` where CPython uses `^`; the test harness normalizes between
them). Frame names use `<module>` for top-level code.
