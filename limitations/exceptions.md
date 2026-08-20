# Exceptions

Monty implements a fixed set of exception classes, listed below. Sandboxed
code **cannot define new exception classes**: the `class` statement exists
(see ./classes.md) but classes cannot inherit, so there is no way
to subclass `BaseException`/`Exception`. `raise` must therefore use one of
these built-ins; `raise MyClass()` on a plain user class raises
`TypeError: exceptions must derive from BaseException`, as in CPython.

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
`ExceptionGroup` / `BaseExceptionGroup` (see ./language.md).

## Constructor signature

All exception constructors accept **zero or one string argument** only.
Multi-argument forms used in CPython (e.g. `OSError(errno, strerror,
filename)`, `UnicodeDecodeError(encoding, obj, start, end, reason)`) are
not supported; passing more than one argument raises an internal error.

## Attributes

- `exc.args` — a tuple with 0 or 1 elements. Always a `tuple`, even when
  empty.
- `str(exc)` — returns the single message string, or `""` if none.
- `repr(exc)` — `ClassName('message')` matching CPython, **except**
  `UnicodeDecodeError`/`UnicodeEncodeError`: CPython reprs these from their
  real 5-field constructor (`UnicodeDecodeError('ascii', b'\xff', 0, 1,
  'ordinal not in range(128)')`), which Monty doesn't track, so Monty's
  `repr()` uses the generic single-message form instead.

**Not implemented:** `__cause__`, `__context__`, `__suppress_context__`,
`__traceback__`, `__notes__`, `add_note()`. The `raise X from Y` syntax
parses, but the `from Y` cause is **silently dropped**: chained
tracebacks are not preserved across `raise from`.

## Custom subclasses

User `class` definitions are supported, but classes cannot inherit
(`class Foo(Exception):` raises `NotImplementedError: ... class inheritance
and metaclasses`), so there is no way to create a new exception class inside
the sandbox. Raising a plain user class instance (`raise MyClass()`) fails
with `TypeError: exceptions must derive from BaseException`. Define custom
exception types on the host side if needed, or use the built-in subclass
that best fits.

## Control flow in `finally`

`break`/`continue`/`return` inside a `finally` block follows CPython
semantics (the finally body runs exactly once and a `return`/`break`/
`continue` that exits it discards any in-flight exception), but Monty does
not emit CPython 3.14's PEP 765 `SyntaxWarning` for such statements, having
no warnings machinery.

## Traceback behaviour

Tracebacks are formatted to match CPython, including the
`File "...", line N, in <function>` lines and `~` caret markers (Monty
uses `~` where CPython uses `^`; the test harness normalizes between
them). Frame names use `<module>` for top-level code.

Known caret divergences:

- CPython suppresses carets on a frame whose location is exactly the call in a
  simple `name = f(...)` assignment or `return f(...)` statement (a noise
  heuristic in `traceback._should_show_carets`); Monty always draws carets for
  the frame's range.
- For a frame whose location spans multiple lines (e.g. a caller frame covering
  a whole multi-line `class` statement), Monty renders the CPython-style source
  block (all lines when the range covers at most three, otherwise
  `...<N lines>...` elision) but never draws caret markers under it, where
  CPython draws multi-line carets for partial-line ranges such as a multi-line
  binary expression.

Monty never emits CPython's `Did you mean: '...'?` suggestions on
`NameError`/`AttributeError`. This divergence is invisible to the test
suite: `scripts/run_traceback.py` strips the suggestions from CPython's output
before comparison, so traceback tests cannot catch it.

An exception raised inside a Python callable that native code invokes
*synchronously* — the `key=`/predicate/function argument of `map`, `filter`,
`sorted`/`min`/`max`, and a user-defined
`__iter__`/`__next__`/`__contains__`/`__repr__`/`__str__` — omits the **calling**
frame from its traceback; the callee frame is present.
CPython shows both. The re-entrant call path (`evaluate_function`) does not
splice the host call site into the traceback. The exception type and message
are unaffected.
