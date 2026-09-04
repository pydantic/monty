# Snapshots

Monty can pause mid-execution, serialize the whole interpreter to bytes, and resume it later — in another process, or on
another machine.

It works because the sandbox holds no operating system resources.
There are no live file descriptors, no sockets and no threads to reconstruct: when execution suspends, everything that
matters is in the interpreter's own heap.

## Two things you can snapshot

|                  | Taken when                     | Restored with   | Contains                                     |
| ---------------- | ------------------------------ | --------------- | -------------------------------------------- |
| **Session dump** | between feeds, nothing running | `load_session`  | globals, functions, classes, time budget     |
| **Snapshot**     | mid-feed, at a suspension      | `load_snapshot` | all of the above, plus the paused call stack |

Both come from `dump()` and are opaque bytes.
Using the wrong loader for a dump's kind raises, and both loaders are valid only on a fresh session, before any feed.

## Pausing at suspensions

`feed_start` is the suspendable counterpart of `feed_run`.
Instead of driving a snippet to completion it hands control back at every suspension:

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

### The snapshot kinds

| Kind                 | Why execution stopped                                                                                                                      | Resume with                                                                                                      |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------- |
| `FunctionSnapshot`   | A host function or OS call, or with `object_id` set a method call on a [host object](host-objects.md) (construction arrives as `__call__`) | `resume(result)`, `resume_not_handled()`, `resume_auto()`                                                        |
| `NameLookupSnapshot` | An undefined name was read, or with `object_id` set a lazy attribute of a host object                                                      | `resume(value=...)`, `resume()` to raise `NameError` (`AttributeError` when `object_id` is set), `resume_auto()` |
| `FutureSnapshot`     | Every sandbox task is blocked on host futures                                                                                              | `resume({call_id: result})`                                                                                      |
| `MontyComplete`      | Nothing — the snippet finished                                                                                                             | nothing; read `.output`                                                                                          |

`FunctionSnapshot.resume` accepts four shapes of answer:

- `{'return_value': value}` — the call returned this.
- `{'exception': ValueError('...')}` — the call raised this exception instance.
- `{'exc_type': 'ValueError', 'message': '...'}` — the call raised this exception, named by type.
    Useful when you do not have the original exception object, for example when resuming a snapshot that was created
    elsewhere.
- `{'future': ...}` — the call returns a pending future the sandbox can `await`; settle it later at the resulting
    `FutureSnapshot`.

In JavaScript those are separate methods: `resume(value)`, `resumeError(err)` and `resumeFuture()`.

Each snapshot resumes at most once.

### Driving automatically

To iterate to completion without answering each suspension by hand, pass an `external_lookup` (and an `os=` handler if
you need one) to `feed_start` and drive with `resume_auto()`.
It resolves each suspension the same way `feed_run` would, one step at a time, so you can inspect or `dump()` each one
along the way:

=== "Python"

    ```python
    from pydantic_monty import Monty, MontyComplete

    with Monty() as pool:
        with pool.checkout() as session:
            snapshot = session.feed_start(
                'greet(name) + "!"',
                inputs={'name': 'Ada'},
                external_lookup={'greet': lambda n: f'hello {n}'},
            )
            while not isinstance(snapshot, MontyComplete):
                snapshot = snapshot.resume_auto()
            print(snapshot.output)
            #> hello Ada!
    ```

=== "TypeScript"

    ```ts
    import { Monty, MontyComplete } from '@pydantic/monty'

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    let snapshot = await session.feedStart('greet(name) + "!"', {
      inputs: { name: 'Ada' },
      externalLookup: { greet: (n: string) => `hello ${n}` },
    })
    while (!(snapshot instanceof MontyComplete)) {
      snapshot = await snapshot.resumeAuto()
    }
    console.log(snapshot.output) // hello Ada!
    ```

`external_lookup` and `os` passed to `feed_start` are captured **for `resume_auto()` only**.
The initial drive still surfaces every external call and name lookup as a snapshot, and a plain `resume(...)` ignores
them.

## Storing and restoring

`snapshot.dump()` serializes the paused worker.
A fresh session's `load_snapshot` restores it and returns the snapshot to resume:

=== "Python"

    ```python
    from pydantic_monty import FunctionSnapshot, Monty, MontyComplete

    with Monty() as pool:
        with pool.checkout() as session:
            snapshot = session.feed_start(
                'fetch(url)', inputs={'url': 'https://example.com'}
            )
            blob = snapshot.dump()

        # later — restore into a fresh session and resume
        with pool.checkout() as session:
            snapshot = session.load_snapshot(blob)
            assert isinstance(snapshot, FunctionSnapshot)
            result = snapshot.resume({'return_value': 'page contents'})
            assert isinstance(result, MontyComplete)
            print(result.output)
            #> page contents
    ```

=== "TypeScript"

    ```ts
    import { FunctionSnapshot, Monty, MontyComplete } from '@pydantic/monty'

    await using pool = await Monty.create()
    let blob: Buffer
    {
      await using session = await pool.checkout()
      const snapshot = await session.feedStart('fetch(url)', { inputs: { url: 'https://example.com' } })
      if (!(snapshot instanceof FunctionSnapshot)) throw new Error('expected a function call')
      blob = await snapshot.dump()
    }

    // later — restore into a fresh session and resume
    {
      await using session = await pool.checkout()
      const snapshot = await session.loadSnapshot(blob)
      if (!(snapshot instanceof FunctionSnapshot)) throw new Error('expected a function call')
      const result = await snapshot.resume('page contents')
      if (!(result instanceof MontyComplete)) throw new Error('expected completion')
      console.log(result.output) // page contents
    }
    ```

`session.dump()` between feeds serializes an idle session instead; restore it with `session.load_session(blob)` and keep
feeding:

=== "Python"

    ```python
    from pydantic_monty import Monty

    with Monty() as pool:
        with pool.checkout() as session:
            session.feed_run('x = 40')
            blob = session.dump()

        with pool.checkout() as session:
            session.load_session(blob)
            print(session.feed_run('x + 2'))
            #> 42
    ```

=== "TypeScript"

    ```ts
    import { Monty } from '@pydantic/monty'

    await using pool = await Monty.create()
    let blob: Buffer
    {
      await using session = await pool.checkout()
      await session.feedRun('x = 40')
      blob = await session.dump()
    }

    {
      await using session = await pool.checkout()
      await session.loadSession(blob)
      console.log(await session.feedRun('x + 2')) // 42
    }
    ```

## What restoring does and does not carry

- **The dump carries its own configuration.** `script_name`, resource limits and type-check state come from the dump,
    not from the `checkout()` that restored it.
- **The instance store does not travel.** Host objects sent before the dump are unknown to the restored session: they
    come back as `MontyClassProxy` (a host class, `type(x)` included, as `MontyClassTypeProxy` in Python and as a plain
    `{ __monty_type__: 'Type', ... }` marker in JavaScript), method calls on them
    raise `RuntimeError`, lazy attributes raise `AttributeError`, and `ClassType` construction raises `RuntimeError`.
    See [`limitations/pool-architecture.md`](limitations/pool-architecture.md#host-api-behaviour-notes).
- **The accumulated time budget travels with the dump**, so a restored session resumes where it left off rather than
    getting a fresh budget.
- **Only the suspension limit travels.** A restored session keeps `max_suspensions`, but the pool resets its count to
    zero, and a `max_suspensions` set on the restoring `checkout()` caps the dump's.
- **Mounts do not travel.** Host paths are never part of a dump.
    Pass the same `mount=` to `load_snapshot`, or the restored feed's filesystem calls degrade into unhandled OS calls.
    Any `'overlay'` writes made before the dump are gone — the restored overlay starts empty.
- **A restored `FutureSnapshot` cannot be driven with `resume_auto()`.** Its pending coroutines lived in the previous
    process.
    Resolve them by hand with `resume({call_id: ...})`.
- **Dumps are version-specific.** The bytes are Monty's own dump format, a `MONTY\0` magic followed by a dump-format
    version, and a build that reads a different version refuses them, so treat dumps as valid only within a single Monty
    version.
    The same bytes load in-process, in a subprocess and over WebSocket.

## Async

`AsyncMonty` sessions expose the same `feed_start`, `load_session`, `load_snapshot` and `dump`, with awaitable
`resume(...)` and `resume_auto()`.
A coroutine host function answered by `resume_auto()` is awaited concurrently: it yields an `AsyncFutureSnapshot` whose
`resume_auto()` settles the pending coroutines.

The sync `FutureSnapshot.resume_auto()` always raises — a sync session cannot drive coroutine host functions.

## Rust

In Rust the in-process API serializes through the free function `monty::dump`, which takes an idle or suspended session
by reference, and `Dump::load`, which returns the session plus its script name and type-check state.
Through `monty-pool` it is `Checkout::dump` and `Checkout::restore`.
See the [Rust quickstart](quickstart/rust.md#serialization).

## Uses

- **Long-running agents.** Suspend at a tool call, persist the blob, resume when the tool answers, possibly on a
    different host.
- **Approval gates.** Pause at a sensitive call, store the snapshot, resume once a human approves.
- **Forking.** One snapshot restored into several sessions explores several branches from the same state.
- **Surviving restarts.** A remote server draining for deploy answers with a dump you can restore elsewhere.
