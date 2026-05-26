# `with` statement (context managers)

Monty supports the `with` statement for built-in types that implement
`__enter__` / `__exit__` (currently just file objects produced by
[`open()`](open.md); user-defined classes are not yet supported anywhere in
Monty). Semantics follow CPython for the supported subset: `__enter__` runs
before the body, `__exit__` runs on every exit path (normal completion,
exception, `return`, `break`, `continue`), and a truthy return from
`__exit__` suppresses an in-flight exception.

## Supported but desugared

- **Multiple context managers in a single `with`** (`with a() as x, b() as y:`)
  is parsed as semantically equivalent nested `with` blocks: the leftmost
  manager enters first and exits last. This matches CPython's left-to-right
  enter, right-to-left exit ordering exactly. Tracebacks point at the
  inner-most `with` line, not the original multi-item line.

## Not supported

- **Async `with`** (`async with EXPR:`) is rejected at parse time with
  `SyntaxError: async context managers (async with) is not yet implemented`.
- **User-defined classes** cannot define `__enter__` / `__exit__` because
  `class` definitions are not yet implemented in Monty
  (`SyntaxError: class definitions is not yet implemented`). Only built-in
  types can be context managers.
- **`contextlib`** (`@contextmanager`, `ExitStack`, etc.) — the module is not
  available; only the language-level `with` statement is.

## Behavioural divergences

- The third argument to `__exit__` (the traceback object) is always `None`.
  Monty has no traceback objects; the type and value arguments are passed
  through unchanged. Code that inspects the traceback object inside `__exit__`
  will see `None` where CPython would provide a `traceback` instance.
- If `__exit__` itself raises during the exception path, the new exception
  replaces the original (the original is dropped). This matches CPython's
  behavior, but is called out here because some readers expect the original
  to be preserved as `__context__` — Monty does not currently track exception
  chaining.

## Current implementers of the protocol

| Type        | Notes                                                            |
| ----------- | ---------------------------------------------------------------- |
| `open()`    | Closes the file on exit; see [`open.md`](open.md) for details.   |

Adding a new context-manager-capable built-in is a matter of overriding
`PyTrait::py_enter` / `PyTrait::py_exit` on the type's `HeapRead` impl, and
threading the `Enter`/`Exit` static-string dispatch through that type's
`py_call_attr` if direct `obj.__enter__()` invocation should also work.
