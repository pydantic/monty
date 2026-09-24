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
If the exception includes a dump, restore it into a fresh session using the appropriate
[snapshot loader](../../snapshots.md#storing-and-restoring).
Restoring a suspended call re-announces it even if the host already executed the callback, so callback side effects
can happen twice.
Neither exception occurs on the local subprocess transport.

## Stored sessions

A server that stores sessions gives each one an opaque ID instead of sending its state back.
[`session_id`][pydantic_monty.AsyncMontySession.session_id] holds it; it is `None` against a server that stores nothing.
Pass it to [`load_session()`][pydantic_monty.AsyncMontySession.load_session] or
[`load_snapshot()`][pydantic_monty.AsyncMontySession.load_snapshot] on a fresh session, from any process, to claim the
session.
With `fork=True` the server copies it under a new ID instead, leaving the original in place.
Against such a server [`dump()`][pydantic_monty.AsyncMontySession.dump] returns a snapshot ID, which loads as a copy.
`checkout(ephemeral=True)` asks the server never to store the session, so it has no ID and `dump()` is refused.

When such a server drains a session, the client redials, reclaims the session by its ID and re-sends the request, so
the caller sees the result rather than `MontyShutdown`.
`MontyShutdown` still surfaces if the reclaim fails, if `auto_resume=False` is passed to
[`AsyncMontyWebsocket`][pydantic_monty.AsyncMontyWebsocket], or if the session has no ID.
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
