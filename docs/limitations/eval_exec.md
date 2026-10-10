# eval(), exec() and locals()

`eval(source, /, globals=None, locals=None)` and `exec(source, /, globals=None, locals=None, *, closure=None)` compile
`source` when called and run it in the namespace CPython would: the module globals for a call at module scope, a
snapshot of the function's locals (PEP 667) plus the module globals inside a function, or the dicts passed as
arguments.
Functions defined by a snippet under a `globals` dict resolve their globals through that dict at every call:
`ns = {'x': 1}; exec('def f(): return x', ns); ns['x'] = 2; ns['f']()` returns `2`.
The snippet can call host functions and raise into the caller; top-level `await`, including in class bodies, is rejected.

## Arguments

- `source` must be a `str` or UTF-8 `bytes`.
    There are no code objects and no `compile()`, so a code object cannot be passed.
    `closure` must be `None` (the default); non-`None` values raise `TypeError`.
- `globals` defaults to `None`, using the caller's globals; non-`None` values must be a `dict`.
- `locals` defaults to `None`, using `globals` when an explicit globals dict is passed, or the caller's locals otherwise.
    Non-`None` values must be a `dict`; CPython accepts any mapping.

## Namespace divergences

- **`__builtins__` is never inserted into a `globals` dict.** `exec('x = 1', ns)` leaves `ns == {'x': 1}`, where
    CPython adds a `'__builtins__'` entry; reading it in Monty raises `NameError` unless it was explicitly supplied or assigned.
- **Module dunders under a `globals` dict raise `NameError`** unless the dict defines them.
    CPython resolves them through the `builtins` module, so `exec('print(__name__)', {})` prints `builtins`.
    Without a `globals` dict the snippet uses Monty's read-only [module-level dunders](language.md#module-level-dunder-variables),
    so assigning to them, including through a `global` declaration, raises `NotImplementedError`.
    The assignment is rejected before any snippet statement runs; its traceback points to the assignment's line.
    This restriction does not apply to explicit globals dictionaries.
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

Parsing and compilation count against `max_feed_duration` and `max_turn_duration`.
Once a snippet starts executing, its source, literals, functions and bytecode remain allocated for the rest of the
session, counted against `max_memory`, even if execution raises an exception.
A snippet rejected before execution, including one refused by the recursion limit, retains none of its compilation
products.
See [resource_limits.md](resource_limits.md).
