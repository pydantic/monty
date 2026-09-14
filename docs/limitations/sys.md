# `sys` module

The module exposes only the attributes listed below; every other `sys.*`
access raises `AttributeError`.

## Attributes

- `sys.version` — the string `"3.14.0 (Monty)"`.
- `sys.version_info` — named tuple `(major=3, minor=14, micro=0, releaselevel='final', serial=0)`.
- `sys.hexversion` — `0x030E00F0`, the packed form of that `version_info`.
- `sys.api_version` — `1013`, CPython 3.14's C API version. Monty has no C API,
    so nothing can be loaded against it; the number is reported only so
    version-gated code reads what it expects.
- `sys.argv` — a one-element list holding the script name, with any host
    directory component stripped (`['main.py']`), so `sys.argv[0]` is what
    `__file__` places under the working directory. Monty runs no command line,
    so there is never an argument after `argv[0]`; passing arguments from the
    host is not supported yet. The list is mutable as in CPython, but `import`
    builds a fresh module every time, so edits to it do not survive a second
    `import sys` (see [modules.md](modules.md)).
- `sys.platform` — the string `"monty"`, not `"linux"` / `"darwin"` /
    `"win32"`. Code that branches on the host OS will not work; the sandbox
    does not expose which OS it runs on.
- `sys.copyright` — Monty's copyright line, not CPython's.
- `sys.builtin_module_names` — every module Monty can import, since they are
    all compiled into the interpreter. CPython lists only its C modules, so the
    tuple differs: it includes `json`, `re`, `typing` and the other modules that
    are pure Python in CPython, and omits `builtins`, `time` and everything else
    Monty does not implement. See [modules.md](modules.md) for the module list.
- `sys.maxsize` — `2**63 - 1` on every target, including 32-bit wasm where the
    real container ceiling is lower. Resource limits bind long before either.
- `sys.maxunicode` — `1114111` (`U+10FFFF`), as in CPython.
- `sys.byteorder` — always `"little"`; Monty builds for no big-endian target.
- `sys.float_info` — the IEEE 754 binary64 properties of the `f64` Monty stores
    floats in, so the values match CPython. `rounds` is `1` (round-to-nearest)
    and nothing in the sandbox can change it.
- `sys.float_repr_style` — always `"short"`.
- `sys.executable`, `sys.prefix`, `sys.exec_prefix`, `sys.base_prefix`,
    `sys.base_exec_prefix` — the empty string. The sandbox has no install tree,
    and CPython documents the empty string for a path it cannot determine.
    `prefix == base_prefix`, so the usual virtualenv test reports "not in one".
- `sys.platlibdir` — `"lib"`, joined onto a `sys.prefix` that is empty.
- `sys.abiflags` — the empty string; Monty has no ABI. CPython does not define
    this attribute at all on Windows.
- `sys.dont_write_bytecode` — `True`, where CPython defaults to `False`. Monty
    never writes bytecode to disk.
- `sys.pycache_prefix` — always `None`.
- `sys.flags` — the same 18 fields CPython 3.14 reports, in the same order.
    Monty is started with no command line, so every switch reads `0` / `False`,
    including `utf8_mode` and `safe_path` even though the sandbox has no other
    encoding and no `sys.path`. Two fields describe Monty rather than an unset
    switch: `dont_write_bytecode` is `1`, agreeing with
    `sys.dont_write_bytecode`, and `hash_randomization` is `0` because Monty
    seeds no hashes — CPython reports `1` by default. `int_max_str_digits` is
    `4300`, the limit Monty enforces, but there is no
    `sys.set_int_max_str_digits` to change it. CPython 3.14 carries three
    further fields *outside* the sequence — `gil`, `thread_inherit_context` and
    `context_aware_warnings`, reachable by name but not by index and not
    counted by `len()`. They describe the GIL and thread-context machinery
    Monty does not have, so they raise `AttributeError`; the 18-element
    sequence itself matches CPython's.
- `sys.stdout` / `sys.stderr` — opaque marker objects with no methods. They
    cannot be written to via `.write()`, and `sys.stdout.flush()` and the rest
    raise `AttributeError`. They are useful only as `print(..., file=...)`,
    which routes output to that stream through the host print callback (see
    [print.md](print.md)).

Accessing an attribute the module does not define raises Monty's generic
`AttributeError: 'module' object has no attribute '<name>'`, where CPython says
`module 'sys' has no attribute '<name>'`.

## Not implemented

`path`, `modules`, `exit`, `exc_info`, `getrecursionlimit`,
`getsizeof`, `getrefcount`, `intern`, `displayhook`, `excepthook`,
`settrace`, `setprofile`, `stdin`, `__stdout__`, `_getframe`, `audit`.

`hash_info`, `int_info` and `thread_info` describe CPython's own C
implementation — its string and integer hashing, its bignum digit layout, its
thread library — none of which Monty shares, so they raise `AttributeError`
rather than reporting fabricated values. In particular `hash_info` cannot be
used to predict Monty's hashes, which differ from CPython's (see
[builtins.md](builtins.md)).

`ps1` and `ps2` are absent, so `hasattr(sys, 'ps1')` correctly reports that the
sandbox is not an interactive prompt.

The private install-layout attributes `_base_executable`, `_framework`, `_git`,
`_home` and `_stdlib_dir` are absent for the same reason as `sys.prefix` and
friends: there is no install tree.

Production builds do not expose `sys.setrecursionlimit`. Test builds expose a
lowering-only hook so shared fixtures can force deterministic recursion errors;
it cannot raise the host-configured ceiling. See
[resource_limits.md](resource_limits.md).
