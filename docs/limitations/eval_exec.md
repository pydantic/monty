# eval(), exec() and locals()

`eval(source, /, globals=None, locals=None)` and `exec(source, /, globals=None, locals=None, *, closure=None)` compile
`source` when called and run it in the namespace CPython would: the module globals for a call at module scope, a
snapshot of the function's locals (PEP 667) plus the module globals inside a function, or the dicts passed as
arguments.
Functions defined by a snippet under a `globals` dict resolve their globals through that dict at every call, so
`ns = {}; exec(code, ns); ns['f']()` works as in CPython.
The snippet runs as a frame of its own: it can call host functions, `await`, and raise into the caller.

## Arguments

- `source` must be a `str` or UTF-8 `bytes`.
    There are no code objects and no `compile()`, so a code object cannot be passed; `closure=` raises `TypeError`.
- `globals` must be a `dict`.
- `locals` must be a `dict`; CPython accepts any mapping.

## Namespace divergences

- **`__builtins__` is never inserted into a `globals` dict.** `exec('x = 1', ns)` leaves `ns == {'x': 1}`, where
    CPython adds a `'__builtins__'` entry, and a snippet that reads `__builtins__` raises `NameError`.
- **Module dunders under a `globals` dict raise `NameError`** unless the dict defines them.
    CPython resolves them through the `builtins` module, so `exec('print(__name__)', {})` prints `builtins`.
    Without a `globals` dict the snippet reads the [module-level dunders](language.md#module-level-dunder-variables) as
    compiled code does.
- **A host-served name first read inside a snippet is cached as a module global**, as for any other read of an undefined
    global; see [name lookups](../host-functions.md).
    A snippet run under a `globals` dict never asks the host: only the dict and the builtins are consulted, which is what
    CPython does with `exec(source, {})`.
- **`del name` does not parse** anywhere, snippets included (see [language.md](language.md)).

## Errors

- A `SyntaxError` in the snippet carries CPython's `(<string>, line N)` suffix, but the message before it is the
    parser's own wording, and the traceback has no extra `File "<string>", line N` frame for the syntax error.
- Frames of snippet code appear in tracebacks as `File "<string>", line N, in <module>` with no source line, as in
    CPython.

## locals()

- At module scope `locals()` returns a fresh `dict` of the bound module globals, not the module namespace itself:
    writes to it are not reflected, `locals() is locals()` is `False`, and the module-level dunders are absent.
- Inside a function it is a snapshot of the named locals, with captured variables read through their cells, as in
    CPython 3.13 and later.
    A function with both `*args` and keyword-only parameters lists `*args` before the keyword-only parameters; CPython
    lists the keyword-only parameters first.
- Inside a snippet it is the snippet's `locals` dict, or its `globals` dict when the two are the same, as in CPython.

## Type checking

Snippet source is never type-checked.
A session with type checking enabled checks the code it is fed; `eval()` and `exec()` compile their strings at
runtime, where no checker runs, so a call a host function stub would reject is only caught by the host function itself.

## Resource use

Every call parses and compiles inside the VM, so the work is charged against `max_duration`, and the call's source,
one `<string>` filename string and its bytecode stay allocated for the rest of the session, counted against
`max_memory`.
See [resource_limits.md](resource_limits.md).
