# `sys` module

Minimal. The module exposes only the attributes listed below; every other
`sys.*` access raises `AttributeError`.

## Attributes

- `sys.version` — the string `"3.14.0 (Monty)"`.
- `sys.version_info` — named tuple `(major=3, minor=14, micro=0,
  releaselevel='final', serial=0)`.
- `sys.platform` — the string `"monty"` (not `"linux"` / `"darwin"` /
  `"win32"`). Code that branches on the host OS will not work; the
  sandbox deliberately hides it.
- `sys.stdout` / `sys.stderr` — opaque marker objects with no methods.
  They cannot be written to via `.write()`; printing always goes through
  the host print callback regardless.

## Not implemented

`argv`, `path`, `modules`, `prefix`, `executable`, `byteorder`,
`maxsize`, `maxunicode`, `flags`, `float_info`, `int_info`, `hash_info`,
`exit`, `exc_info`, `getrecursionlimit`,
`getsizeof`, `getrefcount`, `intern`, `displayhook`, `excepthook`,
`settrace`, `setprofile`, `stdin`, `__stdout__`, `_getframe`, `audit`.

Production builds do not expose `sys.setrecursionlimit`. Test builds expose a
lowering-only hook so shared fixtures can force deterministic recursion errors;
it cannot raise the host-configured ceiling. See
[resource_limits.md](resource_limits.md).
