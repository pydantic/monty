# `asyncio` module and `async` / `await`

`async def` functions can suspend on `await`, and the host drives long-running
external calls. The sandbox schedules its own tasks; host event loops execute external coroutines.

## Module surface

The `asyncio` module exposes these functions, the `Task` type and one exception:

- `asyncio.run(coro)` — runs a coroutine to completion. Returns the value
    the coroutine `return`s, or re-raises an exception from it.
- `asyncio.create_task(coro, *, name=None)` — schedules a coroutine, or adopts
    a pending host future, as an `asyncio.Task`. See
    [below](#asynciotask-and-create_task).
- `asyncio.gather(*awaitables)` — runs awaitables concurrently and returns
    a list of results. Always behaves as `return_exceptions=False`.
    Accepts coroutines, gather futures, wait futures, host futures and `Task`s.
    Any keyword argument is rejected with
    `NotImplementedError: gather() does not yet support keyword arguments`,
    where CPython raises
    `TypeError: gather() got an unexpected keyword argument 'X'` because
    `return_exceptions` is a real kwarg there.
- `asyncio.wait(fs, *, timeout=None, return_when=ALL_COMPLETED)` — waits for
    `Task`s and host futures, returning `(done, pending)` sets. See
    [below](#asynciowait).
- `asyncio.sleep(delay, result=None)` — waits as the session's `sleep` setting
    says, then produces `result`. See
    [below](#asynciosleep-waits-at-the-call-not-at-the-await).
- `asyncio.Task` — the type returned by `create_task`; constructing one
    directly is not supported.
- `asyncio.InvalidStateError` — raised by `Task.result()` and `Task.exception()`
    while the task is pending.
- `asyncio.FIRST_COMPLETED`, `asyncio.FIRST_EXCEPTION` and
    `asyncio.ALL_COMPLETED` — the `wait` completion constants.

Not implemented (raise `AttributeError`):

`wait_for`, `shield`, `to_thread`,
`new_event_loop`, `get_event_loop`, `get_running_loop`, `current_task`,
`all_tasks`, `Queue`, `Lock`,
`Semaphore`, `Event`, `Future`, `TaskGroup`, `timeout`,
`timeout_at`, `Timeout`, `as_completed`, `iscoroutine`, `ensure_future`,
the whole `asyncio.subprocess` / `asyncio.streams` / `asyncio.protocols`
surface.

`asyncio.timeout()` / `asyncio.timeout_at()` would be unreachable in any
case: they are async context managers, and `async with` is rejected at parse
time (see [language.md](language.md)).

## `async def` / `await`

- `async def` functions and `await` work; coroutines can call each other.
- **Coroutines are single-shot.** Awaiting the same coroutine object twice
    raises `RuntimeError`. Store the *result*, not the coroutine, if you need
    it again.
- `await` on a non-awaitable raises `TypeError`.
- `async for` and `async with` are **rejected at parse time** (see
    [language.md](language.md)). Async iteration and async context-manager
    protocols do not exist.
- Async comprehensions (`[x async for x in ...]`) are rejected at parse
    time.
- There is no `__await__` protocol. Awaitables are only the things Monty
    knows internally: coroutines from `async def`, gather futures, wait futures,
    `Task`s, and external function call futures returned by host bindings.

## `asyncio.Task` and `create_task`

`create_task(coro, *, name=None)` wraps a coroutine in a `Task` and makes it
runnable; the body first runs when the scheduler next switches tasks, not during
the call. A task can be awaited from several coroutines, and awaiting a finished
task returns its cached result (or re-raises its cached exception).

`name` is converted with `str()`. `get_name()` and `set_name()` work, and
`repr()` follows CPython's `<Task pending name='...'>` /
`<Task finished name='...'>` shape. `done()`, `result()`, `exception()` and
`cancelled()` exist. `cancelled()` is always `False`: Monty never cancels a
task. `cancel()` raises `NotImplementedError` instead of stopping the task.

`context=` and `eager_start=` are rejected as unexpected keyword arguments;
`create_task` accepts only `name`.

These `Task` attributes raise `AttributeError`: `cancelling`, `uncancel`,
`set_result`, `set_exception`, `add_done_callback`, `remove_done_callback`,
`get_stack`, `print_stack`, `get_loop` and `get_context`. (`add_done_callback`
appears in CPython's single-awaitable `wait()` error, which Monty reproduces
verbatim.)

Automatic names are `Task-N`, starting at `Task-2` because the running task is
`Task-1`. `gather()` does not create `Task` objects, so its children consume no
numbers: after `gather(f(), f())`, CPython's next `create_task` is `Task-4`
while Monty's is `Task-2`.

A host future returned by an external binding is also accepted. It is adopted
rather than spawned: resolving the host call completes the `Task`. In Monty such
a future's `type(...).__name__` is `coroutine`, where CPython reports `Future`;
`wait`'s single-awaitable error still names it `Future`, matching CPython.
An already-resolved or already-failed host future starts the task in that state.

A `wait()` future is accepted too. It is adopted like a host future: creating
the task commits the wait immediately, and the wait result completes the
`Task`. A catchable invalid wait is stored as the task's failure and raised
when the task is awaited, not from `create_task`; a terminal error such as a
resource limit aborts `create_task` instead.

Constructing `asyncio.Task(...)` directly is unsupported.

## `asyncio.wait`

`wait(fs, *, timeout=None, return_when=ALL_COMPLETED)` returns `(done, pending)`
sets of `Task`s and host futures. `fs` is truth-tested when the future is first
awaited: a falsy argument raises
`ValueError: Set of Tasks/Futures is empty.` before iteration, matching
CPython's `if not fs`. A truthy empty iterator is collected and then raises the
same error here, where CPython raises `AssertionError: Set of Futures is empty.`

Only `Task`s and host futures are accepted. A `gather()` future is rejected,
where CPython accepts any `Future`. A single awaitable raises
`TypeError: expect a list of futures, not <type>`. A coroutine inside the
iterable raises
`TypeError: Passing coroutines is forbidden, use tasks explicitly.`, and any
other element fails with
`AttributeError: '<type>' object has no attribute 'add_done_callback'`.
Duplicate inputs are deduplicated by identity.

`return_when` accepts values equal to the three module constants, matching
CPython's membership test; any other value raises
`ValueError: Invalid return_when value: <value>`. A timed-out wait
returns the tasks still pending instead of raising. `timeout` is converted and
its timer starts when `wait()` is called, not at the first `await`, matching
the early-start behaviour of [`sleep`](#asynciosleep-waits-at-the-call-not-at-the-await).
Zero, negative and NaN timeouts expire immediately. Under the `'system'` sleep
setting the timeout is capped like a sleep: ten seconds by default, so
`timeout=3600` fires after ten seconds unless the host raises the cap.

A host rejection of the timeout timer counts as an elapsed timeout whether the
host answers with a future or an inline sleep result.

The returned value is a single-use wait future, like CPython's `wait` coroutine:
awaiting it twice raises `RuntimeError: cannot reuse already awaited coroutine`.
Tasks in the `fs` list may be waited again in later `wait` calls.

Completion sets can differ when several inputs become ready before the waiter
resumes. CPython drains every ready callback before returning, so it may report
all of them in `done`; Monty resumes the waiter after the first, so another
ready input can still be in `pending` and not yet `done()`. Awaiting it normally
finishes it.

## `asyncio.sleep()` waits at the call, not at the `await`

CPython's `asyncio.sleep()` returns a coroutine that does nothing until it is
awaited. Monty starts waiting at the call; `await` produces `result` when the wait finishes.
The session's `sleep` setting determines concurrency (see [time.md](time.md)):

- `'system'`: all pools answer with futures, so gathered sleeps overlap and sibling tasks can run meanwhile.
    Standard Rust execution and the CLI wait inline, making gathered sleeps sequential.
    Other ready tasks run after the inline wait returns, not during it.
    An immediately awaited sleep may be answered eagerly if no other task can run.
- `'call_host'`: async handlers in `AsyncMonty` and JavaScript allow gathered sleeps to overlap.
    `OSAccess` is async by default under `AsyncMonty`.
    Sync handlers, including `Monty` callbacks, wait sequentially; results are unchanged.
- `'zero'`: the awaitable settles immediately without yielding to sibling tasks, unlike CPython's `sleep(0)`.
    Zero-delay system sleeps also settle without a round trip.
    Under `'call_host'`, a handler returning a pending future allows sibling tasks to run even for zero delay.

Suspending sleeps count against `max_suspensions`; system sleeps also count against `max_total_sleep`.
Waiting consumes neither execution-time limit (see [time.md](time.md)).

What follows from waiting at the call:

- `asyncio.sleep(...)` whose result is never awaited has still started the
    wait, where CPython runs nothing and warns that the coroutine was never
    awaited.
- A bad `delay` raises at the call rather than at the `await`. The error is the
    one CPython's `delay <= 0` produces —
    `TypeError: '<=' not supported between instances of 'str' and 'int'` — but it
    surfaces one step earlier.
- The value is a future rather than a coroutine, though `type(...).__name__`
    is `coroutine` either way. Its `repr()` is `<coroutine external_future(N)>`,
    not CPython's `<coroutine object sleep at 0x...>`, and awaiting it a second
    time replays the same result where CPython raises
    `RuntimeError: cannot reuse already awaited coroutine`.

`delay` accepts only real numbers, matching CPython's `delay <= 0`: an
`__index__`-able class is rejected here although `time.sleep()` accepts it.
A negative delay waits zero seconds instead of raising, as CPython
effectively does, and one past ~9223372036.85 seconds is clamped to that
maximum rather than raising the `OverflowError` `time.sleep()` raises (see
[time.md](time.md)).
A NaN delay raises CPython's `ValueError: Invalid delay: NaN (not a number)`,
but at the call rather than at the `await`.

## Concurrency model

Concurrency is cooperative and host-driven. `gather` suspends Monty whenever
every branch is blocked on an external call, hands the pending calls to the
host, and resumes when the host returns results. There is no preemption, no
threads and no exposed event loop.

### Siblings left running by a failed `gather` only advance while something else suspends

When one child of a `gather` raises, the siblings keep running as they do in CPython.
They resume only when a host result arrives or when another task awaits: Monty's scheduler runs ready tasks only at
those suspension points and has no idle loop that would otherwise turn.
Code that catches the error and then returns without awaiting again leaves them parked where they were:

```python test="skip"
import asyncio


async def tick(): ...  # awaits a host call


async def raises():
    raise ValueError('x')


async def sibling():
    await asyncio.gather(tick())
    print('sibling finished')


async def main():
    try:
        await asyncio.gather(sibling(), raises())
    except ValueError:
        pass


asyncio.run(main())
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

```python test="skip"
import asyncio


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
`MemoryError: memory limit exceeded`, which sandboxed code cannot catch (see [resource_limits.md](resource_limits.md)).
The nesting is not charged against the recursion limit in either interpreter, and a session with no memory limit has
no bound to hit.
