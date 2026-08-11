# `with` statement (context managers)

Monty supports the `with` statement for built-in types that implement
`__enter__` / `__exit__` (currently just file objects produced by
[`open()`](open.md)) **and for user-defined classes** that define the two
dunders. Semantics follow CPython for the supported subset: `__enter__` runs
before the body, `__exit__` runs on every exit path (normal completion,
exception, `return`, `break`, `continue`), and a truthy return from
`__exit__` suppresses an in-flight exception.

User-class `__enter__` / `__exit__` run as real frames, so — unlike
`__repr__` / `__str__` (see [classes.md](classes.md)) — they may suspend on
external/OS calls and resume mid-`with`. The protocol check matches CPython:
a value whose class lacks `__exit__` raises `TypeError: '...' object does
not support the context manager protocol (missed __exit__ method)`; one with
`__exit__` but no `__enter__` gets the `(missed __enter__ method)` variant.
Lookup is type-level, as in CPython: an instance attribute named
`__enter__`/`__exit__` is ignored by the `with` statement (but used by an
explicit `obj.__enter__()` call, which is an ordinary method call).

## Supported but desugared

- **Multiple context managers in a single `with`** (`with a() as x, b() as y:`)
  is parsed as semantically equivalent nested `with` blocks: the leftmost
  manager enters first and exits last. This matches CPython's left-to-right
  enter, right-to-left exit ordering exactly. Tracebacks point at the
  inner-most `with` line, not the original multi-item line.

  Each extra item counts against the parser's nesting budget as if it were
  written as explicitly nested `with` blocks — see language.md

## Not supported

- **Async `with`** (`async with EXPR:`) is rejected at parse time with
  `SyntaxError: async context managers (async with) is not yet implemented`.
- **`contextlib`** (`@contextmanager`, `ExitStack`, etc.) — the module is not
  available; only the language-level `with` statement is.

## Behavioural divergences

- The third argument to `__exit__` (the traceback object) is always `None`.
  Monty has no traceback objects; the type and value arguments are passed
  through unchanged (`typ is ValueError` etc. works). Code that inspects the
  traceback object inside `__exit__` will see `None` where CPython would
  provide a `traceback` instance.
- If `__exit__` itself raises during the exception path, the new exception
  replaces the original (the original is dropped). This matches CPython's
  behavior, but is called out here because some readers expect the original
  to be preserved as `__context__` — Monty does not currently track exception
  chaining.
- CPython's `BEFORE_WITH` looks up **and binds** `__enter__`/`__exit__` once,
  when the `with` statement is entered; Monty's `WithExit`/`WithExceptStart`
  opcodes look `__exit__` up again when the block exits. A class whose
  `__exit__` attribute is reassigned *during* the body calls the new
  function in Monty but the originally-bound one in CPython (and a
  reassignment that removes it raises `AttributeError` in Monty where
  CPython would still call the original).
- Direct `obj.__exit__(typ, val, tb)` invocation on a **built-in** context
  manager forwards `val` to the type's `py_exit` only when it is `None` or a
  heap-allocated value (matching CPython for the `None` / exception-instance
  cases real callers use). A non-`None` *scalar* `val` (e.g.
  `f.__exit__(int, 5, None)`) cannot be expressed through the internal
  `Option<HeapId>` abstraction and is treated as if `val` were `None` —
  every built-in context manager ignores `val`'s content beyond `is None`,
  so this is not observable in practice. User-class instances are exempt:
  their explicit dunder calls are ordinary method calls and receive all
  three arguments verbatim.
- Direct `obj.__exit__(...)` on a **built-in** context manager requires
  **exactly three** positional arguments — any other arity raises
  `TypeError`. CPython's file/`IOBase.__exit__` is declared `*args` and
  accepts any number, so `f.__exit__()` returns `None` there but raises in
  Monty. (User-class `__exit__` is an ordinary method, so its arity matches
  whatever the user defined, exactly as in CPython.)

## Current implementers of the protocol

| Type           | Notes                                                          |
| -------------- | -------------------------------------------------------------- |
| `open()`       | Closes the file on exit; see [`open.md`](open.md) for details. |
| user classes   | Class must define `__exit__` (and `__enter__`); see above.     |

Adding a new context-manager-capable built-in requires three pieces on the
type's `HeapRead` impl:

1. Override `PyTrait::py_is_context_manager` to return `true` — this is
   what the `BeforeWith` opcode checks to raise CPython's specific
   `TypeError` for non-CM values, *before* `py_enter` runs.
2. Override `PyTrait::py_enter` / `PyTrait::py_exit`.
3. Add the type's arms in `HeapReadOutput::py_is_context_manager`,
   `py_enter`, and `py_exit` (in `heap_data.rs`) so the dispatch
   reaches the overridden methods.

Direct `obj.__enter__()` / `obj.__exit__(...)` invocation on built-in types
is wired centrally in `VM::call_attr` via `dispatch_dunder`, so no per-type
`StaticStrings::Enter` / `StaticStrings::Exit` arms are needed in the
type's `py_call_attr`. Instances skip that interception and use normal
method dispatch.
