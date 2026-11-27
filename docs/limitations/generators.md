# Generators

Generator functions and expressions, `yield`, `yield from`, `send()` and `StopIteration.value` are supported.
See [comprehensions.md](comprehensions.md) for restrictions on generator expression targets and clauses.

## Missing methods and attributes

- `throw()` and `close()` raise `AttributeError`, e.g. `'generator' object has no attribute 'close'`.
    `GeneratorExit` is not an exception class in Monty either (see [exceptions.md](exceptions.md)).
- `gi_frame`, `gi_code`, `gi_running`, `gi_suspended`, `gi_yieldfrom`, `__name__` and `__qualname__` raise
    `AttributeError`.
- `send()` and `__next__()` work only as calls: reading `g.send` without calling it raises `AttributeError`, as the
    dunders of other built-in iterators do (see [iter.md](iter.md)).
- `repr()` uses the function's name, not its qualified name: `<generator object inner at 0x...>` where CPython gives
    `<generator object outer.<locals>.inner at 0x...>`.

## Behaviour that differs

- **An unfinished generator never runs its pending `finally` blocks or `with` exits.**
    CPython closes a generator that is garbage collected, raising `GeneratorExit` at the paused `yield`.
    Monty releases its references without resuming it.
- **Consumers that advance an iterator in Rust cannot suspend.**
    `*` unpacking (including `asyncio.gather(*(f(x) for x in xs))`), `list()`, `tuple()`, `sum()`, `sorted()`,
    `str.join()` and the `itertools` adaptors run the generator to completion without returning to the host, so a host
    call in its body raises
    `NotImplementedError: external function 'name' cannot suspend in this synchronous context` inside the generator,
    where it can be caught.
    Advance the generator with `for`, a list, set or dict comprehension, `next()`, `send()` or `yield from` to allow suspension.
    The same restriction applies to user `__next__` methods; see [iter.md](iter.md).
- **`yield from` cannot advance a delegate that would suspend outside a Python frame.**
    A class whose `__next__` is a host function, or a file that has not been read yet, raises
    `NotImplementedError: yield from: this iterator cannot suspend to the host` inside the delegating generator.
    A `__next__` written in Python may call host functions.
- **The `RuntimeError` for a `StopIteration` escaping a generator (PEP 479) loses its exception chain** when a consumer
    that advances in Rust drove the generator, as for any exception rebuilt after a nested synchronous call (see
    [exceptions.md](exceptions.md)).
    Under `next()` or `for`, both `__cause__` and `__context__` are the original `StopIteration`.
- **`StopIteration.value` is lost when the exception is rebuilt from a copy.**
    A `StopIteration` that escapes a nested synchronous call (a `sorted()` key, a `__repr__`, a generator driven by
    `list()`) reaches the outer handler with `.value` set to its message, or `None`, and `args` to match:
    `sorted([1], key=lambda x: next(g()))` for a `g` that returns `[1, 2]` gives `e.value is None` and `e.args == ()`.
    A string value survives as the message, but not its identity.
- **An uncaught `StopIteration` shows its value only when it is a string.**
    The message a host receives is the exception's text, so `next(g())` escaping the sandbox for a `g` that returns
    `[1, 2]` reports `StopIteration` with no message, where CPython prints `StopIteration: [1, 2]`.
    Inside the sandbox, `str(e)`, `repr(e)` and `e.args` match CPython.
- **A host function that raises `StopIteration(5)` gives the sandbox `e.value == '5'`**, a string, because only the
    message crosses the host boundary.

## Rejected at parse time

- Async generator functions (`async def` containing `yield`) raise `NotImplementedError`.
    Async generator expressions (`async for` or `await` in the deferred body) also raise `NotImplementedError`.
    `yield from` inside `async def` raises CPython's `SyntaxError: 'yield from' inside async function`.

## Tracebacks

- A frame paused at a `yield from` statement that fills its line shows carets under the statement, which CPython
    omits; `x = yield from inner()` matches CPython.
