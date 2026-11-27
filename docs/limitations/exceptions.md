# Exceptions

Monty implements a fixed set of exception classes, listed below. Sandboxed
code **cannot define new exception classes**: classes can inherit only from
classes defined in Python and `object` (see [classes.md](classes.md#inheritance)), so there
is no way to subclass `BaseException`/`Exception`. `raise` must therefore use one of
these built-ins; `raise MyClass()` on a plain user class raises
`TypeError: exceptions must derive from BaseException`, as in CPython.

## Implemented exception classes

`BaseException`, `Exception`, `SystemExit`, `KeyboardInterrupt`,
`ArithmeticError`, `OverflowError`, `ZeroDivisionError`, `LookupError`,
`IndexError`, `KeyError`, `RuntimeError`, `NotImplementedError`,
`RecursionError`, `AttributeError`, `NameError`,
`UnboundLocalError`, `ValueError`, `UnicodeDecodeError`, `UnicodeEncodeError`,
`ImportError`, `ModuleNotFoundError`, `OSError`, `FileNotFoundError`, `FileExistsError`,
`IsADirectoryError`, `NotADirectoryError`, `PermissionError`,
`AssertionError`, `MemoryError`, `StopIteration`, `SyntaxError`,
`TimeoutError`, `TypeError`.

Module-specific: `json.JSONDecodeError` and `tomllib.TOMLDecodeError` (subclasses of `ValueError`),
`re.PatternError` / `re.error`, `io.UnsupportedOperation` (catchable as
both `OSError` and `ValueError`, matching CPython's dual parentage),
`dataclasses.FrozenInstanceError` (a subclass of `AttributeError`) and
`asyncio.InvalidStateError` (a subclass of `Exception`). Hosts see the last one
under CPython's defining-module name, `asyncio.exceptions.InvalidStateError`.

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

## Constructor and attribute divergences

Sandbox-side instances of `OSError` (`errno`, `strerror`, `filename`), `SyntaxError` (`msg`, `filename`, `lineno`, `offset`, `text`), `UnicodeDecodeError`/`UnicodeEncodeError` (`encoding`, `object`, `start`, `end`, `reason`), and `JSONDecodeError`/`TOMLDecodeError` (`msg`, `doc`, `pos`, `lineno`, `colno`) do not validate CPython's specific constructor signatures or expose those per-type attributes inside the sandbox.
VM-raised `AttributeError`, `NameError`, `UnboundLocalError`, `ImportError`, and `ModuleNotFoundError` leave `.name`, `.obj`, and `.path` as `None` rather than populating the offending symbol or module name.

`KeyError` carries the missing key's `str()` text rather than the key itself, so `str(exc)` and `repr(exc)` quote non-string keys: `{}[1]` gives `KeyError('1')` where CPython gives `KeyError(1)`, and a `bytes` key gives `KeyError("b'a'")`.
The key's `__str__` runs when the `KeyError` is raised, which CPython never does.
An exception from it is replaced by the type name (`KeyError('<K>')`), but a resource limit hit inside it ends the run with `MemoryError` or `TimeoutError` where CPython raises a plain `KeyError`.

`repr(exc)` uses the generic args form; VM-raised `UnicodeDecodeError` and `UnicodeEncodeError` therefore differ from CPython's five-field repr, while missing-key `KeyError` and dotted exception classes have the divergences described above.

The dotted exception classes — `json.JSONDecodeError`, `tomllib.TOMLDecodeError`, `re.PatternError`, `binascii.Error`, `binascii.Incomplete`, `_csv.Error`, `statistics.StatisticsError`, `asyncio.exceptions.InvalidStateError` — repr under their qualified name: `binascii.Error('bad')` where CPython gives `Error('bad')`.

An exception rebuilt from a copy loses its chain: one that escapes a nested synchronous call (a `sorted()` key, `json.dumps(default=...)`, a `__repr__`, a generator driven by `list()`) reaches the outer handler with `__cause__` reset to `None`, `__suppress_context__` to `False`, and `__context__` set to the exception the *caller* was handling, if any, rather than the one the callee was handling.

A chain that forms a cycle (`raise e from e`) is only reclaimed when the session ends, since exceptions are not cycle-collected.

**Not implemented:** `__traceback__`, `__notes__`, `add_note()`.
A traceback printed for an exception with a `__cause__` or `__context__` shows that exception alone: CPython's chained output (`The above exception was the direct cause of the following exception:`) is not produced, inside the sandbox or at the host boundary.

## Custom subclasses

User `class` definitions are supported, but a class cannot derive from an
exception type (`class Foo(Exception):` raises `TypeError: Monty does not support subclassing 'Exception': ...`),
so there is no way to create a new exception class inside the sandbox. Raising a plain user class instance (`raise MyClass()`) fails
with `TypeError: exceptions must derive from BaseException`. Define custom
exception types on the host side if needed, or use the built-in subclass
that best fits.

## Control flow in `finally`

`break`/`continue`/`return` inside a `finally` block follows CPython
semantics (the finally body runs exactly once and a `return`/`break`/
`continue` that exits it discards any in-flight exception), but Monty does
not emit CPython 3.14's PEP 765 `SyntaxWarning` for such statements, having
no warnings machinery.

## Attribute errors on type objects

`list.nonexistent` raises `AttributeError: type object 'list' has no attribute 'nonexistent'`, naming the class rather
than the metaclass, and calling it
(`list.nonexistent()`) reports the same message. Both use Monty's name for the
class, which differs from CPython's for one builtin: `pathlib.Path` reports
`PosixPath`, because Monty has a single type where CPython has the `Path`
class and its `PosixPath` instances (see [pathlib.md](pathlib.md)).

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
