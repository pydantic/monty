# Type Checking

Monty supports modern Python type hints and bundles [ty](https://docs.astral.sh/ty/), Astral's type checker, in the same
binary.
There is nothing extra to install or configure.

Type checking is optional and off by default.
Turn it on per session:

=== "Python"

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

=== "TypeScript"

    ```ts
    import { Monty, MontyTypingError } from '@pydantic/monty'

    await using pool = await Monty.create()
    await using session = await pool.checkout({ typeCheck: true })
    try {
      await session.feedRun("x: int = 'not an int'")
    } catch (err) {
      if (!(err instanceof MontyTypingError)) throw err
      console.log(err.display().includes('invalid-assignment')) // true
    }
    ```

The snippet does not run, and the session survives — fix the code and feed again.

## Why it matters more here than usual

Monty implements a [deliberately small subset](limitations/index.md) of Python.
A model that writes `import random` produces code that is perfectly valid CPython and completely unrunnable here.

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

=== "Python"

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

=== "TypeScript"

    ```ts
    import { Monty } from '@pydantic/monty'

    const stubs = `
    def get_temperature(city: str) -> float: ...
    `

    function getTemperature(city: string): number {
      return 21.5
    }

    await using pool = await Monty.create()
    await using session = await pool.checkout({ typeCheck: true, typeCheckStubs: stubs })
    const result = await session.feedRun("get_temperature('London') * 2", {
      externalLookup: { get_temperature: getTemperature },
    })
    console.log(result) // 43
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

=== "Python"

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

=== "TypeScript"

    ```ts
    import { Monty, MontyTypingError } from '@pydantic/monty'

    await using pool = await Monty.create()
    await using session = await pool.checkout({ typeCheck: true })
    await session.feedRun('def double(n: int) -> int:\n    return n * 2')
    try {
      await session.feedRun("double('three')")
    } catch (err) {
      if (!(err instanceof MontyTypingError)) throw err
      console.log(err.display().includes('invalid-argument-type')) // true
    }
    ```

A snippet that fails the check never runs, so it never enters the accumulated context.

Set `skip_type_check=True` on an individual `feed_run` or `feed_start` (`skipTypeCheck` in JavaScript) to bypass
checking for that feed only.

## Reading the diagnostics

[`MontyTypingError.display()`][pydantic_monty.MontyTypingError.display] returns ty's rendered output — source context, underlines and rule names, one diagnostic
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
diagnostic formats — and `type_check_color` (`typeCheckColor` in JavaScript) colours it with ANSI escapes; on the CLI
the flag is `--type-check-format`.

## Elsewhere

- **JavaScript**: `pool.checkout({ typeCheck: true, typeCheckStubs: '...' })`, raising [`MontyTypingError`][pydantic_monty.MontyTypingError] with the same
    `.display()`.
- **Rust**: [`ReplConfig`](api/rust/monty-pool.md#replconfig) on `monty-pool`, or the [`monty-type-checking`](https://crates.io/crates/monty-type-checking)
    crate directly.
- **CLI**: `monty --type-check file.py`.
    See [command line](cli.md).

## Caveats

- **Type checking is static only.** The `typing` module inside the sandbox provides markers, not runtime enforcement —
    no annotation is ever checked at runtime, and class annotations are stored in stringized form.
    See [`limitations/typing.md`](limitations/typing.md).
- **Passing the type check does not mean the code runs.** Parser-rejected constructs (`match`, `yield`) are not
    modelled.
    Five stub-only modules (`abc`, `types`, `typing_extensions`, `_collections_abc`, `_typeshed`) resolve during checking
    because the stubs need them, then raise `ModuleNotFoundError` at runtime.
    See [`limitations/modules.md`](limitations/modules.md).
