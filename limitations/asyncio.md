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
Among the tasks that do resume, only the ordering differs.
This applies to all async code, not only to siblings of a failed `gather`.

### `gather` does not start its children until it is awaited

CPython's `gather` schedules each awaitable as a task straight away, so the children run whether or not the result is
ever awaited.
Monty holds the awaitables and spawns nothing until the `await`, so a `gather(...)` whose result is discarded runs no
code at all:

```python
async def boom():
    print('boom ran')
    raise ValueError('x')

asyncio.gather(boom())  # CPython prints `boom ran`, Monty runs nothing
```

Awaiting the gather later still runs the children, and a gather that has already failed re-raises its cached
exception on every later await.

Neither does Monty report the exception CPython loses track of here.
CPython prints `_GatheringFuture exception was never retrieved`, a `future:` line and the traceback to stderr when a
gather holding an unretrieved exception is collected.
In Monty that gather never ran, and no sandboxed code can write to stderr in any case — `print()` rejects its `file`
argument.

### Nesting `gather` inside `gather` is bounded by `max_memory`

CPython nests gathers as deeply as memory allows, and the depth costs nothing beyond the futures themselves.
Monty holds a walk frame per level while it commits the nest, so awaiting a deep nest costs memory on top of what the
nest already occupies.
A session with `max_memory` set can therefore build a nest it cannot await: the `await` ends the run with
`MemoryError: memory limit exceeded`, which sandboxed code cannot catch (see ./resource_limits.md).
The nesting is not charged against the recursion limit in either interpreter, and a session with no memory limit has
no bound to hit.
