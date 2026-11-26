---
# Show individual methods/attributes (h5) in the docs site's on-page TOC.
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 5
---

# Remote Client

A pool of remote `monty` workers reached over gRPC instead of local subprocesses — the intended peer is
`monty-server`.
The pool holds one HTTP/2 connection to the server and each checkout opens one `MontySession.Session` stream on it;
a stream is never reused.
See [running monty-server](../../server.md) and the [remote-worker trust boundary](../../security.md#remote-workers).
A remote peer may be CPython rather than a Monty sandbox; the transport does not provide isolation.
[`AsyncMontyWebsocket`][pydantic_monty.AsyncMontyRemote] is the same class under its deprecated former name, and a `ws://` or
`wss://` URL is read as `http://` or `https://`.

`checkout(profile='...')` names a server-side profile to run the session under; `None` takes the server's default.
The client sends the name to the server unchecked; what a profile means is the server's choice, and the `monty` worker
ignores it.

## Pre-warmed sessions

`AsyncMontyRemote(url, pre_warm=2)` keeps two sessions open and ready, so a checkout takes one instead of opening a
stream.
`__aenter__` waits for them to open and raises if one cannot.
`pre_warm_wait=False` opens them in the background instead: a failed open is not raised but retried with a backoff, and
a checkout that finds none ready opens its own stream.
Each one a checkout takes is replaced in the background.
One checkout per session already opening waits for it; further concurrent checkouts open their own.
They share `max_processes` with checked-out sessions, so `pre_warm` above it raises
[`MontyConfigurationError`][pydantic_monty.MontyConfigurationError], and each is a session to the server from the moment
it opens.
`metadata` is called for each pre-warmed session as the pool opens it, from a pool thread without the caller's
contextvars.
A checkout takes a ready session only if its own `metadata` call returned the same value.
Otherwise it closes the ready sessions unused and opens its own, so `pre_warm` helps only while `metadata` is stable.
Leave it off when one pool's checkouts send different metadata, such as per-user credentials.

## Connection loss and shutdown

[`MontyRefusedError`][pydantic_monty.MontyRefusedError] means the server refused to open the session's stream: at
capacity, with no worker to serve it, or rejecting the caller; its `status_code`, `status_message` and `cause` say
why, and no session existed.
`retryable` is true for `RESOURCE_EXHAUSTED` and `UNAVAILABLE`, not for credential errors.
`retry_after` is the suggested delay in seconds from `Retry-After`, capped at one hour.
It is `None` if the hint is absent, invalid or repeated.
The client accepts delay-seconds and HTTP dates.
The client retries a refused checkout once when the server sends a single valid `Retry-After` of at most two seconds, whatever the refusal's status, including when `retryable` is false.
It waits at least 100 ms or the server's delay, whichever is longer, plus up to 250 ms of jitter.
The wait and retry share a five-second budget and respect the original checkout and dial deadlines.
If the retry times out, the client returns the original refusal.
If the server refuses the retry, the client returns that second refusal.
A server that cannot be reached, that answers with any other status (such as `UNIMPLEMENTED` from a URL that is
no Monty server), or that is no gRPC server at all (such as a web server's page), raises
[`MontyConnectError`][pydantic_monty.MontyConnectError].
It retains `status_code`, `status_message` and `cause` when the server sent a status, or `transport_error` otherwise.
Both errors subclass [`MontyError`][pydantic_monty.MontyError].
If a CPU-mode server requires a finite `request_timeout`, an incompatible client setting raises
[`MontyConfigurationError`][pydantic_monty.MontyConfigurationError].
Malformed request metadata names or values raise the same error before any connection attempt.
[`MontyDisconnectError`][pydantic_monty.MontyDisconnectError] means the stream ended mid-session.
A server that ended it by policy can report a cause (see [disconnect diagnostics](#disconnect-diagnostics)).
A transport failure says nothing about the worker's fate.
Either way the session is lost; check out a new one.
The error text includes the recorded code, cause and message, or the transport error.
For example: `monty worker connection closed while waiting for a reply: DEADLINE_EXCEEDED (turn_timeout): turn expired`.
This does not establish whether the request ran, so do not replay it automatically.

[`MontyShutdown`][pydantic_monty.MontyShutdown] means the server declined the next request because it is shutting down.
That request did not run.
Against a server that stores sessions the client resumes the session itself before raising, see
[stored sessions](#stored-sessions); the exception is raised only when that fails or is disabled.
If the exception includes a dump, restore it into a fresh session using the appropriate
[snapshot loader](../../snapshots.md#storing-and-restoring).
Restoring a suspended call re-announces it even if the host already executed the callback, so callback side effects
can happen twice.
Neither exception occurs on the local subprocess transport.

## Trace context

Each [`feed_run()`][pydantic_monty.AsyncMontySession.feed_run],
[`feed_start()`][pydantic_monty.AsyncMontySession.feed_start] and load sends the W3C `traceparent` of the
OpenTelemetry span current where it was called, so a server that records the session parents that feed's spans to it.
A server that records spans names its span for each external call, OS call and name lookup, and the host callback
answering it runs with that span current: a span the callback starts is its child.
With `instrument_telemetry()` the callback runs under Monty's own span for the call instead, which is a child of the
server's.
[`trace_context()`][pydantic_monty.FunctionSnapshot.trace_context] returns the same span for a manual handler.
If the server's spans are not exported to your tracing backend, spans started in host callbacks appear there without
their parent.

This needs only `opentelemetry-api`, not `instrument_telemetry()`.
Nothing is sent when `opentelemetry-api` is not installed or no span is current, and a server that names no span
leaves callbacks in the caller's context.

Against a server that records the session, a tracer passed to `instrument_telemetry()` records every span twice, once
on each side.
Pass only `meter=` (and `logger=`, if you want Monty's logs) to keep client-side metrics without the duplicate spans.
`logfire.instrument_monty()` always installs a tracer, so it records the duplicates.

## Stored sessions

A server that stores sessions gives each one an opaque ID instead of sending its state back.
[`get_session_id()`][pydantic_monty.AsyncMontySession.get_session_id] returns it; it returns `None` against a server
that stores nothing.
The session's configuration normally travels with its first request, so called before any request,
`get_session_id()` sends the configuration on its own and waits for the reply, which costs one round trip.
The `session_id` property is deprecated: it is `None` until the server has accepted the configuration, whose reply
comes back with the first request's, even when that request fails.
`monty-server` run without a store also refuses `dump()`, `load_session()` and `load_snapshot()` with
`MontyRuntimeError`, and the session carries on; the development relay in `scripts/grpc_relay.py` instead passes
them to its worker, which dumps and loads bytes as a local session does.
Against a storing server [`dump()`][pydantic_monty.AsyncMontySession.dump] writes the session's current state to a
record that never changes and returns that record's ID; the session continues under its own ID.
The server also writes the session's state under the session's ID whenever it parks the session: idle for its
`--park-after`, the client gone, or a drain.
Passing either ID to the loader matching the stored state, [`load_session()`][pydantic_monty.AsyncMontySession.load_session]
for an idle session or [`load_snapshot()`][pydantic_monty.AsyncMontySession.load_snapshot] for one suspended mid-feed, on
a fresh session, from any process, starts a new session with its own ID: a session's ID gives the state as of that
session's last park, a `dump()` ID the state dumped.
The record is unchanged and the session that wrote it is never resumed in place, so loading one ID twice gives two
independent sessions, and a session still running elsewhere is unaffected.
Loading an ID restores a dump, so [what restoring does not carry](../../snapshots.md#what-restoring-does-and-does-not-carry)
applies: host objects sent before the dump, in particular, are unknown to the new session.
To branch a session at a chosen point, call `dump()`, then `load_session()` with its ID once per branch; the original
session can carry on as well.
`checkout(ephemeral=True)` asks the server not to store the session on its own, so it has no ID; whether it honours
`dump()` is the server's choice.

When such a server drains a session, the client opens a new stream, loads the state the server named into a new session and
re-sends the request, so the caller sees the result rather than `MontyShutdown`; `get_session_id()` then returns the new
session's ID, and the session's suspension and sleep totals carry over.
`MontyShutdown` is still raised if the server named nothing to load, if the reload fails, if `auto_resume=False` is
passed to [`AsyncMontyRemote`][pydantic_monty.AsyncMontyRemote], or if the session has no ID.
The new stream uses the metadata returned when the session was entered, so an expired token makes
the resume fail.
`MontyDisconnectError` is never resumed, because the request may have run.

## Disconnect diagnostics

[`MontyDisconnectError`][pydantic_monty.MontyDisconnectError] exposes `status_code`, `status_message`, `cause` and
`transport_error`.
If the server ended the stream with a gRPC status, `status_code` holds its canonical name (`DEADLINE_EXCEEDED`,
`RESOURCE_EXHAUSTED`, `UNAVAILABLE`, ...), or `OK` when the server simply finished the stream, and `status_message` its
message.
A `monty-server` names why in `cause`: `idle_timeout`, `session_timeout`, `turn_timeout`, `out_of_memory`,
`server_shutdown`, `capacity`, `caller_cap`, `unauthenticated`, `permission_denied`, `worker_unavailable` or
`protocol_violation`.
An `out_of_memory` cause is raised as the same `MemoryError` a local out-of-memory kill raises, and a
`server_shutdown` cause as [`MontyShutdown`][pydantic_monty.MontyShutdown] with no dump, so neither reaches this error.
A relay can report `turn_timeout` in the failure reply before the client receives the gRPC status.
In this case `status_code`, `status_message` and `transport_error` are `None`.
A connection failure or protocol failure supplies `transport_error` text instead, with no status.
A later failure does not replace an earlier one.
Missing evidence remains `None`, and a status message is server-supplied text, not a verified explanation.
These fields do not establish whether the request ran, so the client does not replay it.

## Dependencies

[`install_dependencies()`][pydantic_monty.AsyncMontySession.install_dependencies] is supported only by embedded-CPython workers.
It installs PEP 508 requirements using `uv`, within the pool's `request_timeout`.
Those workers also install PEP 723 inline dependencies before running a feed.
A Monty sandbox worker rejects non-empty installation requests with `MontyRuntimeError` and ignores PEP 723 comments.
`install_dependencies([])` is a no-op on either worker.

## Host decode budget

`AsyncMontyRemote(max_decode_bytes=...)` can lower the default 1 GiB allocation budget for each decoded protocol frame.
Values above 1 GiB are rejected when the client is constructed.
The budget excludes wire buffers and Python conversion, and does not change
sandbox memory limits. Exhaustion ends the session with a protocol error;
the client does not replay the request.
Separately, the transport refuses any message larger than the budget, clamped between 64 KiB and the 256 MiB frame limit, before buffering it.

## API

::: pydantic_monty
    options:
        members:
            - AsyncMontyRemote
