---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Websocket Client

A pool of remote `monty` workers reached over a WebSocket instead of local subprocesses — the intended peer is
`monty-server`.
See [running monty-server](../../server.md) and the [remote-worker trust boundary](../../security.md#remote-workers).
A remote peer may be CPython rather than a Monty sandbox; the transport does not provide isolation.

## Connection loss and shutdown

[`MontyDisconnectError`][pydantic_monty.MontyDisconnectError] means the connection closed mid-session.
It does not distinguish a worker crash from a server policy drop; check out a new session.

[`MontyShutdown`][pydantic_monty.MontyShutdown] means the server declined the next request because it is shutting down.
That request did not run.
Against a server that stores sessions the client resumes the session itself before raising, see
[stored sessions](#stored-sessions); the exception is raised only when that fails or is disabled.
If the exception includes a dump, restore it into a fresh session using the appropriate
[snapshot loader](../../snapshots.md#storing-and-restoring).
Restoring a suspended call re-announces it even if the host already executed the callback, so callback side effects
can happen twice.
Neither exception occurs on the local subprocess transport.

## Stored sessions

A server that stores sessions gives each one an opaque ID instead of sending its state back.
[`session_id`][pydantic_monty.AsyncMontySession.session_id] holds it; it is `None` against a server that stores nothing,
which also refuses `dump()`, `load_session()` and `load_snapshot()` with `MontyRuntimeError` (the session carries on).
Against such a server [`dump()`][pydantic_monty.AsyncMontySession.dump] writes the session's current state to a
record that never changes and returns that record's ID; the session continues under its `session_id`.
The server also writes the session's state under `session_id` whenever it parks the session: idle for its
`--park-after`, the client gone, or a drain.
Passing either ID to the loader matching the stored state, [`load_session()`][pydantic_monty.AsyncMontySession.load_session]
for an idle session or [`load_snapshot()`][pydantic_monty.AsyncMontySession.load_snapshot] for one suspended mid-feed, on
a fresh session, from any process, starts a new session with its own ID: a `session_id` gives the state as of that
session's last park, a `dump()` ID the state dumped.
The record is unchanged and the session that wrote it is never resumed in place, so loading one ID twice gives two
independent sessions, and a session still running elsewhere is unaffected.
Loading an ID restores a dump, so [what restoring does not carry](../../snapshots.md#what-restoring-does-and-does-not-carry)
applies: host objects sent before the dump, in particular, are unknown to the new session.
To branch a session at a chosen point, call `dump()`, then `load_session()` with its ID once per branch; the original
session can carry on as well.
`checkout(ephemeral=True)` asks the server not to store the session on its own, so it has no ID; whether it honours
`dump()` is the server's choice.

When such a server drains a session, the client redials, loads the state the server named into a new session and
re-sends the request, so the caller sees the result rather than `MontyShutdown`; `session_id` then names the new
session, and the session's suspension and sleep totals carry over.
`MontyShutdown` is still raised if the server named nothing to load, if the reload fails, if `auto_resume=False` is
passed to [`AsyncMontyWebsocket`][pydantic_monty.AsyncMontyWebsocket], or if the session has no ID.
The redial uses the headers `connect_headers` returned when the session was entered, so an expired token makes the
resume fail.
`MontyDisconnectError` is never resumed, because the request may have run.

## Dependencies

[`install_dependencies()`][pydantic_monty.AsyncMontySession.install_dependencies] is supported only by embedded-CPython workers.
It installs PEP 508 requirements using `uv`, within the pool's `request_timeout`.
Those workers also install PEP 723 inline dependencies before running a feed.
A Monty sandbox worker rejects non-empty installation requests with `MontyRuntimeError` and ignores PEP 723 comments.
`install_dependencies([])` is a no-op on either worker.

## API

::: pydantic_monty
    options:
        members:
            - AsyncMontyWebsocket
