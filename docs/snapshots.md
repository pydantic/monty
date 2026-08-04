# Snapshots

Monty can pause mid-execution, serialize the whole interpreter to bytes, and resume it
later — in another process, or on another machine. This is unusual for a Python runtime
and it is one of the main reasons Monty exists.

It works because the sandbox holds no operating system resources. There are no live file
descriptors, no sockets and no threads to reconstruct: when execution suspends, everything
that matters is in the interpreter's own heap.

## Two things you can snapshot

| | Taken when | Restored with | Contains |
| --- | --- | --- | --- |
| **Session dump** | between feeds, nothing running | `load_session` | globals, functions, classes, accumulated time budget |
| **Snapshot** | mid-feed, at a suspension | `load_snapshot` | all of the above, plus the paused call stack |

Both come from `dump()` and are opaque bytes. Using the wrong loader for a dump's kind
raises, and both loaders are valid only on a fresh session, before any feed.

## Pausing at suspensions

`feed_start` is the suspendable counterpart of `feed_run`. Instead of driving a snippet to
completion it hands control back at every suspension:

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

### The snapshot kinds

| Kind | Why execution stopped | Resume with |
| --- | --- | --- |
| `FunctionSnapshot` | A [host function](host-functions.md) or OS call | `resume(result)`, `resume_not_handled()` (OS calls), `resume_auto()` |
| `NameLookupSnapshot` | An undefined name was read | `resume(value=...)`, or `resume()` to leave it undefined and raise `NameError` |
| `FutureSnapshot` | Every sandbox task is blocked on host futures | `resume({call_id: result})` |
| `MontyComplete` | Nothing — the snippet finished | nothing; read `.output` |

`FunctionSnapshot.resume` accepts three shapes of answer:

- `{'return_value': value}` — the call returned this.
- `{'exception': ValueError('...')}` — the call raised this exception instance.
- `{'exc_type': 'ValueError', 'message': '...'}` — the call raised this exception, named
  by type. Useful when you do not have the original exception object, for example when
  resuming a snapshot that was created elsewhere.

Each snapshot resumes at most once.

### Driving automatically

To iterate to completion without answering each suspension by hand, pass an
`external_lookup` (and an `os=` handler if you need one) to `feed_start` and drive with
`resume_auto()`. It resolves each suspension the same way `feed_run` would, one step at a
time, so you can inspect or `dump()` each one along the way:

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

`external_lookup` and `os` passed to `feed_start` are captured **for `resume_auto()`
only**. The initial drive still surfaces every external call and name lookup as a
snapshot, and a plain `resume(...)` ignores them.

## Storing and restoring

`snapshot.dump()` serializes the paused worker. A fresh session's `load_snapshot` restores
it and returns the snapshot to resume:

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

`session.dump()` between feeds serializes an idle session instead; restore it with
`session.load_session(blob)` and keep feeding:

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

## What restoring does and does not carry

- **The dump carries its own configuration.** `script_name`, resource limits and
  type-check state come from the dump, not from the `checkout()` that restored it. The
  dataclass registry from `checkout()` is reused.
- **The accumulated time budget travels with the dump**, so a restored session resumes
  where it left off rather than getting a fresh budget.
- **Mounts do not travel.** Host paths are never part of a dump. Pass the same
  `mount=` to `load_snapshot`, or the restored feed's filesystem calls degrade into
  unhandled OS calls. Any `'overlay'` writes made before the dump are gone — the restored
  overlay starts empty.
- **A restored `FutureSnapshot` cannot be driven with `resume_auto()`.** Its pending
  coroutines lived in the previous process. Resolve them by hand with
  `resume({call_id: ...})`.
- **Dumps are version- and transport-specific.** The bytes use a subprocess-specific
  envelope and restore only into another worker of the same Monty version.

## Async

`AsyncMonty` sessions expose the same `feed_start`, `load_session`, `load_snapshot` and
`dump`, with awaitable `resume(...)` and `resume_auto()`. A coroutine host function
answered by `resume_auto()` is awaited concurrently: it yields an `AsyncFutureSnapshot`
whose `resume_auto()` settles the pending coroutines.

The sync `FutureSnapshot.resume_auto()` always raises — a sync session cannot drive
coroutine host functions.

## Rust and JavaScript

In Rust the in-process API dumps and loads directly: a paused `RunProgress` is a
self-contained snapshot, and `MontyRun` itself dumps to cache parsed code. Through
`monty-pool` it is `Checkout::dump` and `Checkout::restore`. See the
[Rust quickstart](quickstart/rust.md#serialization).

In JavaScript the API mirrors Python's: `session.feedStart`, `snapshot.dump()`,
`session.loadSnapshot(blob)`, `session.dump()` and `session.loadSession(blob)`.

## Uses

- **Long-running agents.** Suspend at a tool call, persist the blob, resume when the tool
  answers, possibly on a different host.
- **Approval gates.** Pause at a sensitive call, store the snapshot, resume once a human
  approves.
- **Forking.** One snapshot restored into several sessions explores several branches from
  the same state.
- **Surviving restarts.** A remote server draining for deploy answers with a dump you can
  restore elsewhere.
