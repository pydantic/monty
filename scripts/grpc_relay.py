#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["grpcio>=1.75"]
# ///
"""gRPC ↔ `monty subprocess` relay — the server side of the remote-worker hop.

The monty worker pools can drive a *remote* worker over gRPC instead of a
local subprocess (`pydantic_monty.AsyncMontyRemote`). This script is the
server side of that hop for local development and testing: it serves the
`monty.v1.MontySession/Session` RPC and bridges each session stream to a
fresh `monty subprocess` child.

The bridge only translates framing. The stdio child length-prefixes each
protocol message with 4 little-endian bytes; gRPC carries one message per
frame. So the relay adds the prefix on the way to the child and strips it on
the way back — it never parses the protobuf payload, needs no schema, and
works across monty versions (the child runs its own version-skew check). It
does **not** add any sandboxing: a relayed worker is exactly as isolated as
the child it spawns.

Usage:
    uv run scripts/grpc_relay.py [--host HOST] [--port PORT] [--monty-bin PATH]

`--port 0` binds an ephemeral port. Once listening, the relay prints the chosen
`http://host:port` URL as a single line to stdout, so a caller (e.g. a test)
can read it back before connecting.

The pieces are importable too (`start_relay`, `SessionHandler`, `admit`,
`spawn_child`, `bridge_stream`), so a test can host the same bridge in-process
and wrap it with its own behaviour, e.g. to record each stream's metadata.
"""

from __future__ import annotations

import argparse
import asyncio
import ipaddress
import os
import shutil
import struct
from collections.abc import AsyncIterator, Callable
from typing import Any

import grpc
from grpc import aio

# The stdio child's per-frame header: a 4-byte little-endian length prefix.
_LENGTH_PREFIX = struct.Struct('<I')

# The one RPC the pool calls, from `crates/monty-proto/proto/monty/v1/monty.proto`.
SESSION_METHOD = '/monty.v1.MontySession/Session'

# `MAX_FRAME_LEN` in `monty-proto`, plus a little for gRPC's own framing: the
# relay must never reject a message the protocol itself would accept.
MAX_MESSAGE_BYTES = 256 * 1024 * 1024 + 1024

# A stream handler: the pool's request messages in, the child's events out.
Behaviour = Callable[[AsyncIterator[bytes], aio.ServicerContext[bytes, bytes]], AsyncIterator[bytes]]

# Child frames queued for a gRPC reader. Bounded so a reader slower than a
# print-heavy child fills the queue and then the child's stdout pipe, which
# stalls the child rather than growing the relay's memory.
EVENT_QUEUE_DEPTH = 16


class ChildExited(Exception):
    """The child ended the session by exiting with a non-zero status."""

    def __init__(self, status: int) -> None:
        super().__init__(f'monty subprocess exited with status {status}')
        self.status = status


async def spawn_child(monty_bin: str) -> asyncio.subprocess.Process:
    """Starts a fresh `monty subprocess` child for one stream.

    Separate from the bridge so a handler can spawn *before* admitting the
    stream: a child that cannot start then refuses the checkout rather than
    failing the session.
    """
    return await asyncio.create_subprocess_exec(
        monty_bin,
        'subprocess',
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
    )


async def bridge_stream(requests: AsyncIterator[bytes], child: asyncio.subprocess.Process) -> AsyncIterator[bytes]:
    """Bridges one session stream to `child`, a `monty subprocess`.

    Pumps messages in both directions until either side ends, then tears the
    child down. Each gRPC message is one protocol frame; the same frame is
    length-prefixed on stdio, so the only work is adding/stripping the prefix.
    A child that exits with a non-zero status after its last frame raises
    `ChildExited`, for the handler to end the stream with a status.
    """
    assert child.stdin is not None
    assert child.stdout is not None
    stdin, stdout = child.stdin, child.stdout
    # the child's frames, ending with `None` at its EOF, read by the generator below
    events: asyncio.Queue[bytes | None] = asyncio.Queue(maxsize=EVENT_QUEUE_DEPTH)

    async def requests_to_child() -> None:
        try:
            async for body in requests:
                stdin.write(_LENGTH_PREFIX.pack(len(body)) + body)
                await stdin.drain()
        finally:
            # the pool half-closed, or its requests failed: either way the
            # child reads EOF as a clean end and exits, which ends the stream
            stdin.close()

    async def child_to_events() -> None:
        try:
            while True:
                (length,) = _LENGTH_PREFIX.unpack(await stdout.readexactly(_LENGTH_PREFIX.size))
                await events.put(await stdout.readexactly(length))
        except (asyncio.IncompleteReadError, OSError):
            pass  # the child exited, or its pipe broke: either way it is done
        # not in a `finally`: a cancelled pump has no reader left to tell, and
        # a full queue would hold the cancellation
        await events.put(None)

    pumps = [asyncio.create_task(requests_to_child()), asyncio.create_task(child_to_events())]
    try:
        while (event := await events.get()) is not None:
            yield event
        # the child closed its stdout: it has exited, cleanly or not
        if (status := await child.wait()) != 0:
            raise ChildExited(status)
    finally:
        # Cancel whatever still runs (a cancelled stream lands here too) and
        # reap both so neither is left with an unretrieved exception, then
        # tear the child down.
        for task in pumps:
            task.cancel()
        await asyncio.gather(*pumps, return_exceptions=True)
        await reap(child)


async def reap(child: asyncio.subprocess.Process) -> None:
    """Ends `child` if it still runs, and waits for it either way."""
    if child.returncode is None:
        child.kill()
    await child.wait()


async def admit(context: aio.ServicerContext[bytes, bytes]) -> None:
    """Sends the response headers that admit the stream.

    The pool sends nothing, not even `Configure`, until it sees them, so a
    server that waits for a request before answering deadlocks with it. A
    refusal is the opposite: `context.abort` *before* this, so the client sees
    a trailers-only response and fails the checkout rather than the session.
    """
    await context.send_initial_metadata(())


def relay_behaviour(monty_bin: str) -> Behaviour:
    """The plain relay: every stream gets a fresh child, then is admitted and
    bridged to it. A child that cannot start refuses the stream with
    `UNAVAILABLE`; one that exits with a non-zero status ends it with
    `INTERNAL`, so the pool can tell a crash from a clean close.

    An async generator *function*, which is how grpc's aio server tells a
    streaming handler from a one-shot one.
    """

    async def behaviour(
        requests: AsyncIterator[bytes], context: aio.ServicerContext[bytes, bytes]
    ) -> AsyncIterator[bytes]:
        try:
            child = await spawn_child(monty_bin)
        except OSError as exc:
            await context.abort(grpc.StatusCode.UNAVAILABLE, f'cannot start the monty subprocess: {exc}')
        try:
            await admit(context)
        except BaseException:
            # nothing bridges the child yet, so a failed or cancelled
            # admission would otherwise leave it running
            await reap(child)
            raise
        try:
            async for event in bridge_stream(requests, child):
                yield event
        except ChildExited as exc:
            await context.abort(grpc.StatusCode.INTERNAL, str(exc))

    return behaviour


class SessionHandler(grpc.GenericRpcHandler):
    """Routes the session RPC to `behaviour`, with messages as raw bytes.

    Generic rather than generated: the relay has no schema, so the serializers
    are the identity and the protobuf passes through untouched.
    """

    def __init__(self, behaviour: Behaviour) -> None:
        self._handler: grpc.RpcMethodHandler[bytes, bytes] = grpc.stream_stream_rpc_method_handler(behaviour)

    def service(self, handler_call_details: grpc.HandlerCallDetails) -> grpc.RpcMethodHandler[bytes, bytes] | None:
        return self._handler if handler_call_details.method == SESSION_METHOD else None


async def start_server(handler: grpc.GenericRpcHandler, host: str, port: int) -> tuple[aio.Server, str]:
    """Starts a server for `handler` and returns it with its dialable URL."""
    server = aio.server(
        options=[
            ('grpc.max_receive_message_length', MAX_MESSAGE_BYTES),
            ('grpc.max_send_message_length', MAX_MESSAGE_BYTES),
        ]
    )
    server.add_generic_rpc_handlers((handler,))
    bound = server.add_insecure_port(f'{format_host(host)}:{port}')
    await server.start()
    return server, format_url(host, bound)


async def start_relay(monty_bin: str, host: str = '127.0.0.1', port: int = 0) -> tuple[aio.Server, str]:
    """Starts the plain relay and returns the server with its dialable URL."""
    return await start_server(SessionHandler(relay_behaviour(monty_bin)), host, port)


async def serve_relay(host: str, port: int, monty_bin: str) -> None:
    """Serves the relay until cancelled, printing the bound `http://` URL once up."""
    server, url = await start_relay(monty_bin, host, port)
    print(url, flush=True)
    try:
        await server.wait_for_termination()
    finally:
        await server.stop(None)


def format_url(host: str, port: int) -> str:
    """Builds a dialable `http://` URL from a bound address.

    Handles the two address shapes a bind can produce that a naive
    `http://{host}:{port}` gets wrong: IPv6 literals must be bracketed in a URI
    (`http://[::1]:port`), and a wildcard bind (`0.0.0.0` / `::`) is not a usable
    destination, so it's mapped to the matching loopback address. A plain
    hostname is left untouched.
    """
    try:
        ip = ipaddress.ip_address(host)
    except ValueError:
        return f'http://{host}:{port}'  # a hostname, not an IP literal
    if ip.is_unspecified:
        ip = ipaddress.ip_address('::1' if ip.version == 6 else '127.0.0.1')
    return f'http://{format_host(str(ip))}:{port}'


def format_host(host: str) -> str:
    """Brackets an IPv6 literal for a `host:port` address; anything else as is."""
    try:
        return f'[{host}]' if ipaddress.ip_address(host).version == 6 else host
    except ValueError:
        return host


def resolve_monty_bin(explicit: str | None) -> str:
    """The monty binary to spawn: explicit arg, then `$MONTY_BIN`, then `PATH`."""
    return explicit or os.environ.get('MONTY_BIN') or shutil.which('monty') or 'monty'


def main() -> None:
    parser = argparse.ArgumentParser(description='Bridge gRPC session streams to monty subprocess children.')
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=8799, help='listen port (0 binds an ephemeral port)')
    parser.add_argument('--monty-bin', default=None, help='monty binary path (default: $MONTY_BIN, then PATH)')
    args: Any = parser.parse_args()
    try:
        asyncio.run(serve_relay(args.host, args.port, resolve_monty_bin(args.monty_bin)))
    except KeyboardInterrupt:
        pass


if __name__ == '__main__':
    main()
