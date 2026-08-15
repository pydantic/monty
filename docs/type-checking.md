# Type Checking

Monty supports modern Python type hints and bundles [ty](https://docs.astral.sh/ty/), Astral's type checker, in the same
binary.
There is nothing extra to install or configure.

Type checking is optional and off by default.
Turn it on per session:

```python
from pydantic_monty import Monty, MontyTypingError

with Monty() as pool:
    with pool.checkout(type_check=True) as session:
        try:
            session.feed_run("x: int = 'not an int'")
        except MontyTypingError as exc:
            print('invalid-assignment' in exc.display())
            #> True
```

The snippet does not run, and the session survives — fix the code and feed again.

## Why it matters more here than usual

Monty implements a [deliberately small subset](python-subset.md) of Python.
A model that writes `import functools` produces code that is perfectly valid CPython and completely unrunnable here.

Type checking closes that gap, because Monty does not check against CPython's typeshed.
It checks against [`monty-typeshed`](https://crates.io/crates/monty-typeshed), a trimmed typeshed describing *Monty's*
runtime surface: unsupported modules, builtins and methods are filtered out of the stubs entirely.
Code reaching for something Monty does not implement usually fails the check up front, instead of failing at runtime
halfway through.

For an LLM writing code, that turns a whole class of runtime failures into a diagnostic you can hand straight back to
the model as a retry prompt.

## Declaring what the host provides

Sandboxed code calls [host functions](host-functions.md) that are not defined anywhere in the snippet, so a type checker
has never heard of them.
`type_check_stubs` is where you declare them:

```python
from pydantic_monty import Monty

stubs = """
def get_temperature(city: str) -> float: ...
"""


def get_temperature(city: str) -> float:
    return 21.5


with Monty() as pool:
    with pool.checkout(type_check=True, type_check_stubs=stubs) as session:
        result = session.feed_run(
            "get_temperature('London') * 2",
            external_lookup={'get_temperature': get_temperature},
        )
        print(result)
        #> 43.0
```

The stubs are written alongside the source with a wildcard import injected, and diagnostic line numbers are adjusted
back to the original snippet — so an error points at the line the model wrote, not at an offset.

Stubs are scoped to the checkout.
A later session does not see them.

Passing the same declarations to the model in its prompt, and to `type_check_stubs` here, is the pattern the
[`examples/`](https://github.com/pydantic/monty/tree/main/examples) directory uses: the model sees the tool signatures,
the checker enforces them.

## Sessions accumulate

Each successfully executed snippet is appended to the context used to check subsequent snippets, so a REPL session type
checks as one growing program:

```python
from pydantic_monty import Monty, MontyTypingError

with Monty() as pool:
    with pool.checkout(type_check=True) as session:
        session.feed_run('def double(n: int) -> int:\n    return n * 2')
        try:
            session.feed_run("double('three')")
        except MontyTypingError as exc:
            print('invalid-argument-type' in exc.display())
            #> True
```

A snippet that fails the check never runs, so it never enters the accumulated context.

Set `skip_type_check=True` on an individual `feed_run` or `feed_start` to bypass checking for that feed only.

## Reading the diagnostics

`MontyTypingError.display()` returns ty's rendered output — source context, underlines and rule names, one diagnostic
per block:

```text
error[unsupported-operator]: Unsupported `+` operation
 --> main.py:1:1
  |
1 | "hello" + 1
  | -------^^^-
  | |         |
  | |         Has type `Literal[1]`
  | Has type `Literal["hello"]`
  |
```

`main.py` is the `script_name` from `checkout()`; set it to something meaningful if you show diagnostics to a model or a
user.
Checking runs inside the worker, so the diagnostics arrive as pre-rendered text.
`type_check_format` on `checkout()` selects a different rendering — `'concise'`, `'json'`, `'github'` and the other ty
diagnostic formats; on the CLI the flag is `--type-check-format`.

## Elsewhere

- **JavaScript**: `pool.checkout({ typeCheck: true, typeCheckStubs: '...' })`, raising `MontyTypingError` with the same
  `.display()`.
- **Rust**: `ReplConfig` on `monty-pool`, or the [`monty-type-checking`](https://crates.io/crates/monty-type-checking)
  crate directly.
- **CLI**: `monty --type-check file.py`.
  See [command line](cli.md).

## Caveats

- **Type checking is static only.** The `typing` module inside the sandbox provides markers, not runtime enforcement —
  no annotation is ever checked at runtime, and class annotations are stored in stringized form.
  See [`limitations/typing.md`](https://github.com/pydantic/monty/blob/main/limitations/typing.md).
- **Passing the type check does not mean the code runs.** Parser-rejected constructs (`match`, `yield`) are not
  modelled.
  Five stub-only modules (`abc`, `types`, `typing_extensions`, `_collections_abc`, `_typeshed`) resolve during checking
  because the stubs need them, then raise `ModuleNotFoundError` at runtime.
  See [`limitations/modules.md`](https://github.com/pydantic/monty/blob/main/limitations/modules.md).
