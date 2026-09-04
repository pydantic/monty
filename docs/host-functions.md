# Host Functions

Host functions are one of three ways sandboxed code reaches anything outside the sandbox.
The others, [host objects](host-objects.md) and [filesystem mounts](filesystem.md), are specialised forms of the same
idea.

When sandboxed code reads a name it never defined, execution **suspends**.
The host resolves the name — usually by running a real function — and execution **resumes** with the result.
The interpreter never calls out; it stops and waits to be told what happened.

## The basics

=== "Python"

    ```python
    from pydantic_monty import Monty


    def get_temperature(city: str) -> float:
        return 21.5


    with Monty() as pool:
        with pool.checkout() as session:
            result = session.feed_run(
                "f'{get_temperature(\"London\")} degrees'",
                external_lookup={'get_temperature': get_temperature},
            )
            print(result)
            #> 21.5 degrees
    ```

=== "TypeScript"

    ```ts
    import { Monty } from '@pydantic/monty'

    function getTemperature(city: string): number {
      return 21.5
    }

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const result = await session.feedRun("f'{get_temperature(\"London\")} degrees'", {
      externalLookup: { get_temperature: getTemperature },
    })
    console.log(result) // 21.5 degrees
    ```

`external_lookup` maps names to host values.
A **callable** entry becomes a function the sandbox can call.
Any **other** value is converted and returned when the name is read:

=== "Python"

    ```python
    from pydantic_monty import Monty

    with Monty() as pool:
        with pool.checkout() as session:
            cfg = {'retries': 3}
            print(session.feed_run('cfg["retries"]', external_lookup={'cfg': cfg}))
            #> 3
    ```

=== "TypeScript"

    ```ts
    import { Monty } from '@pydantic/monty'

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const cfg = { retries: 3 }
    console.log(await session.feedRun('cfg["retries"]', { externalLookup: { cfg } })) // 3
    ```

A name that is not in `external_lookup` raises `NameError` inside the sandbox:

=== "Python"

    ```python
    from pydantic_monty import Monty, MontyRuntimeError

    with Monty() as pool:
        with pool.checkout() as session:
            try:
                session.feed_run('missing()')
            except MontyRuntimeError as exc:
                print(exc.display(format='type-msg'))
                #> NameError: name 'missing' is not defined
    ```

=== "TypeScript"

    ```ts
    import { Monty, MontyRuntimeError } from '@pydantic/monty'

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    try {
      await session.feedRun('missing()')
    } catch (err) {
      if (!(err instanceof MontyRuntimeError)) throw err
      console.log(err.display('type-msg')) // NameError: name 'missing' is not defined
    }
    ```

## `inputs` versus `external_lookup`

Both put host values in front of the sandbox, but they differ in *when*:

|              | `inputs`                         | `external_lookup`             |
| ------------ | -------------------------------- | ----------------------------- |
| Bound        | eagerly, before the snippet runs | lazily, when the name is read |
| Converted    | every entry, used or not         | only what the code touches    |
| Callables    | a reference, not a host function | become host functions         |
| Missing name | not applicable                   | `NameError` in the sandbox    |

A name present in both is served by the eager `inputs` binding.

Pass host functions through `external_lookup`.
A callable in `inputs` binds only a reference carrying the callable's `__name__`, and calling it resolves *that* name
through `external_lookup` — so `inputs={'f': double}` alone raises `NameError: name 'double' is not defined` on `f(2)`.

Prefer `inputs` for the small, always-needed values the code was written around, and `external_lookup` for the tool
surface — a model that writes code calling ten of your tools only pays for the ones it actually calls.

## Arguments and return values

Positional and keyword arguments pass through as written:

=== "Python"

    ```python
    from pydantic_monty import Monty


    def report(name, *, level='info', **extra):
        return f'{name} {level} {extra}'


    with Monty() as pool:
        with pool.checkout() as session:
            code = "report('x', level='warn', code=7)"
            print(session.feed_run(code, external_lookup={'report': report}))
            #> x warn {'code': 7}
    ```

=== "TypeScript"

    ```ts
    import { Monty } from '@pydantic/monty'

    function report(name: string, { level = 'info', ...extra }: { level?: string; [key: string]: unknown } = {}) {
      return `${name} ${level} ${JSON.stringify(extra)}`
    }

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const code = "report('x', level='warn', code=7)"
    console.log(await session.feedRun(code, { externalLookup: { report } })) // x warn {"code":7}
    ```

Return values must be types Monty can represent — the same set `inputs` and `external_lookup` accept, listed under
[which values cross the boundary](quickstart/python.md#which-values-cross-the-boundary).
Arguments come the other way, out of the sandbox.
A sandbox-defined class instance arrives as a read-only `MontyClassProxy`; see
[host objects](host-objects.md#sandbox-instances).
A sandbox value with no host equivalent (a class object, a function, a compiled `re` pattern) arrives silently as its
repr *string* rather than raising, so a host function cannot tell it from a sandbox `str` of the same text.

A return value Monty cannot represent does not raise `MontyConversionError`.
It is delivered into the sandbox as `TypeError: Cannot convert X to Monty value`, which sandboxed code can catch;
uncaught, it reaches you as `MontyRuntimeError`.
The same is true of an `os=` callback's return value.
`MontyConversionError` is for host values you hand over up front, in `inputs` or `external_lookup`.

Values are also bounded in shape and size.
Nesting is capped (roughly 48 nested lists, 32 nested dicts, 24 nested class instances), and a wire frame — the value plus
its envelope — is capped at 256 MiB.
Exceeding either fails the call; it does not crash the worker.

## Raising into the sandbox

An exception raised by a host function crosses the boundary and behaves like an ordinary Python exception inside the
sandbox, so sandboxed code can catch it:

=== "Python"

    ```python
    from pydantic_monty import Monty


    def fetch(url: str) -> str:
        raise ValueError('bad url')


    code = """
    try:
        fetch('nope')
    except ValueError as e:
        result = f'caught: {e}'
    result
    """

    with Monty() as pool:
        with pool.checkout() as session:
            print(session.feed_run(code, external_lookup={'fetch': fetch}))
            #> caught: bad url
    ```

=== "TypeScript"

    ```ts
    import { Monty } from '@pydantic/monty'

    function fetch(url: string): string {
      const err = new Error('bad url')
      err.name = 'ValueError' // the sandbox sees the exception type named here
      throw err
    }

    const code = `
    try:
        fetch('nope')
    except ValueError as e:
        result = f'caught: {e}'
    result
    `

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    console.log(await session.feedRun(code, { externalLookup: { fetch } })) // caught: bad url
    ```

If the sandbox does not catch it, `feed_run` raises `MontyRuntimeError` with the sandbox traceback.
Only [the exception types Monty implements](limitations/index.md) can cross; the type name is what carries over, not your
exception class.

## Async host functions

With `AsyncMonty`, callables in `external_lookup` may be coroutine functions.
They are awaited on your event loop, so blocking work inside one blocks it exactly as it would anywhere else in your
async code — what `AsyncMonty` moves off the loop is worker I/O, not your callbacks.
In JavaScript every host function may be async, and there is no separate pool class.
`asyncio.gather` inside the sandbox lets several run concurrently:

=== "Python"

    ```python
    import asyncio

    from pydantic_monty import AsyncMonty


    async def fetch(url: str) -> str:
        await asyncio.sleep(0.01)
        return f'contents of {url}'


    code = """
    import asyncio

    results = await asyncio.gather(fetch('a'), fetch('b'))
    len(results)
    """


    async def main():
        async with AsyncMonty() as pool:
            async with pool.checkout() as session:
                print(await session.feed_run(code, external_lookup={'fetch': fetch}))
                #> 2


    asyncio.run(main())
    ```

=== "TypeScript"

    ```ts
    import { Monty } from '@pydantic/monty'

    async function fetch(url: string): Promise<string> {
      await new Promise((resolve) => setTimeout(resolve, 10))
      return `contents of ${url}`
    }

    const code = `
    import asyncio

    results = await asyncio.gather(fetch('a'), fetch('b'))
    len(results)
    `

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    console.log(await session.feedRun(code, { externalLookup: { fetch } })) // 2
    ```

The sync `Monty` cannot drive coroutine host functions — use `AsyncMonty`, or resolve the pending futures by hand with
[`feed_start`](snapshots.md).

## Driving suspensions yourself

`feed_run` answers every suspension for you and returns only the final value.
`feed_start` hands each suspension back as a snapshot so you can log it, rate-limit it, require approval for it, or
serialize it and continue tomorrow:

=== "Python"

    ```python
    from pydantic_monty import FunctionSnapshot, Monty, MontyComplete

    with Monty() as pool:
        with pool.checkout() as session:
            snapshot = session.feed_start('greet(name) + "!"', inputs={'name': 'Ada'})
            assert isinstance(snapshot, FunctionSnapshot)
            print(snapshot.function_name, snapshot.args)
            #> greet ('Ada',)
            result = snapshot.resume({'return_value': 'hello Ada'})
            assert isinstance(result, MontyComplete)
            print(result.output)
            #> hello Ada!
    ```

=== "TypeScript"

    ```ts
    import { FunctionSnapshot, Monty, MontyComplete } from '@pydantic/monty'

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const snapshot = await session.feedStart('greet(name) + "!"', { inputs: { name: 'Ada' } })
    if (!(snapshot instanceof FunctionSnapshot)) throw new Error('expected a function call')
    console.log(snapshot.functionName, snapshot.args) // greet [ 'Ada' ]
    const result = await snapshot.resume('hello Ada')
    if (!(result instanceof MontyComplete)) throw new Error('expected completion')
    console.log(result.output) // hello Ada!
    ```

`resume` also takes `{'exception': SomeError('...')}` to raise into the sandbox, or `{'exc_type': 'ValueError', 'message': '...'}` when you only have the type by name.
In JavaScript `resume(value)` takes the return value directly and `resumeError(err)` raises.
See [snapshots](snapshots.md) for the full set of snapshot kinds.

## Designing a safe tool surface

Monty guarantees the sandbox reaches nothing you did not hand it.
It cannot guarantee that what you handed it is safe, because a host function runs in your process with your process's
authority.

- **Validate arguments as untrusted input.** A host function taking a path, a URL, a SQL fragment or a shell string is a
    filesystem, network, database or shell primitive that you built.
    The model writing the code is not adversarial by assumption, but the code it writes is not reviewed.
- **Keep the surface narrow.** `read_customer(id)` is a tool; `read_file(path)` is a filesystem.
- **Prefer mounts for file access.** [Mounts](filesystem.md) already enforce canonicalization, boundary checks and
    symlink rejection.
    A hand-rolled `read_file` host function does not.
- **Make callbacks idempotent if you plan to restore snapshots across a restart.** When a suspended session is dumped
    while the host is answering a call, restoring it re-announces that call and it runs again.
