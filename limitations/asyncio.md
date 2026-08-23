# `asyncio` module and `async` / `await`

`async def` functions can suspend on `await`, and the host drives long-running
external calls. There is no event loop inside the sandbox; the host is the
loop.

## Module surface

The `asyncio` module exposes exactly two functions:

- `asyncio.run(coro)` — runs a coroutine to completion. Returns the value
  the coroutine `return`s, or re-raises an exception from it.
- `asyncio.gather(*awaitables)` — runs awaitables concurrently and returns
  a list of results. Always behaves as `return_exceptions=False`.
  Any keyword argument is rejected with
  `NotImplementedError: gather() does not yet support keyword arguments`,
  where CPython raises
  `TypeError: gather() got an unexpected keyword argument 'X'` because
  `return_exceptions` is a real kwarg there.

Not implemented (raise `AttributeError`):

`create_task`, `sleep`, `wait`, `wait_for`, `shield`, `to_thread`,
`new_event_loop`, `get_event_loop`, `get_running_loop`, `Queue`, `Lock`,
`Semaphore`, `Event`, `Future`, `Task`, `TaskGroup`, `timeout`,
`timeout_at`, `Timeout`, `as_completed`, `iscoroutine`, `ensure_future`,
the whole `asyncio.subprocess` / `asyncio.streams` / `asyncio.protocols`
surface.

`asyncio.timeout()` / `asyncio.timeout_at()` would be unreachable in any
case: they are async context managers, and `async with` is rejected at parse
time (see ./language.md).

## `async def` / `await`

- `async def` functions and `await` work; coroutines can call each other.
- **Coroutines are single-shot.** Awaiting the same coroutine object twice
  raises `RuntimeError`. Store the *result*, not the coroutine, if you need
  it again.
- `await` on a non-awaitable raises `TypeError`.
- `async for` and `async with` are **rejected at parse time** (see
  ./language.md). Async iteration and async context-manager
  protocols do not exist.
- Async comprehensions (`[x async for x in ...]`) are rejected at parse
  time.
- There is no `__await__` protocol. Awaitables are only the things Monty
  knows internally: coroutines from `async def`, gather futures, and external
  function call futures returned by host bindings.

## Concurrency model

Concurrency is cooperative and host-driven. `gather` suspends Monty whenever
every branch is blocked on an external call, hands the pending calls to the
host, and resumes when the host returns results. There is no preemption, no
threads, and no in-sandbox scheduler.

### Siblings left running by a failed `gather` only advance while something else suspends

When one child of a `gather` raises, the siblings keep running as they do in CPython.
They resume only when a host result arrives or when another task awaits, because Monty has no event loop of its own
to turn.
Code that catches the error and then returns without awaiting again leaves them parked where they were:

```python
async def sibling():
    await asyncio.gather(tick())
    print('sibling finished')

try:
    await asyncio.gather(sibling(), raises())
except ValueError:
    pass
```

CPython prints `sibling finished` here, Monty prints nothing.

An external call a sibling already passed to the host is still resolved by the host.
Its result reaches the sibling; a result arriving for a `gather` that has already failed is discarded.

Which task runs next also differs.
A task resumed by a host result runs to its next suspension before any other ready task gets a turn, where CPython's
loop takes them in the order they were woken.
Only the interleaving differs: every task still gets its turns, and no result is lost.
This applies to all async code, not only to siblings of a failed `gather`.

### A failed `gather` nobody awaits is never reported

CPython prints `_GatheringFuture exception was never retrieved`, a `future:` line and the traceback to stderr when a
failed `gather` is garbage collected without having been awaited.
Monty prints nothing, and has no stderr to print it to: `print()` only ever writes stdout.
Awaiting the `gather` later still raises the cached exception.
