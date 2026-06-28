#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["websockets>=14"]
# ///
"""WebSocket ↔ `monty subprocess` relay — the server side of the remote-worker hop.

The monty worker pools can drive a *remote* worker over a WebSocket instead of a
local subprocess (`pydantic_monty.AsyncMontyWebsocket`). This script is the
server side of that hop for local development and testing: it accepts WebSocket
connections and bridges each one to a fresh `monty subprocess` child.

The bridge only translates framing. The stdio child length-prefixes each
protocol message with 4 little-endian bytes; the WebSocket transport sends one
binary message per frame. So the relay adds the prefix on the way to the child
and strips it on the way back — it never parses the protobuf payload, needs no
schema, and works across monty versions (the child runs its own version-skew
check). It does **not** add any sandboxing: a relayed worker is exactly as
isolated as the child it spawns.

Usage:
    uv run scripts/websocket_relay.py [--host HOST] [--port PORT] [--monty-bin PATH]

`--port 0` binds an ephemeral port. Once listening, the relay prints the chosen
`ws://host:port` URL as a single line to stdout, so a caller (e.g. a test) can
read it back before connecting.
"""

from __future__ import annotations

import argparse
import asyncio
import ipaddress
import os
import shutil
import struct

from websockets.asyncio.server import ServerConnection, serve
from websockets.exceptions import ConnectionClosed

# The stdio child's per-frame header: a 4-byte little-endian length prefix.
_LENGTH_PREFIX = struct.Struct('<I')


async def bridge_connection(websocket: ServerConnection, monty_bin: str) -> None:
    """Bridges one WebSocket connection to a fresh `monty subprocess` child.

    Pumps frames in both directions until either side closes, then tears the
    child down. Each WebSocket binary message is one protocol frame; the same
    frame is length-prefixed on stdio, so the only work is adding/stripping the
    prefix.
    """
    child = await asyncio.create_subprocess_exec(
        monty_bin,
        'subprocess',
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
    )
    assert child.stdin is not None
    assert child.stdout is not None
    stdin, stdout = child.stdin, child.stdout

    async def ws_to_child() -> None:
        async for message in websocket:
            # the pool only ever sends binary protocol frames
            body = message.encode() if isinstance(message, str) else message
            stdin.write(_LENGTH_PREFIX.pack(len(body)) + body)
            await stdin.drain()
        stdin.close()

    async def child_to_ws() -> None:
        while True:
            (length,) = _LENGTH_PREFIX.unpack(await stdout.readexactly(_LENGTH_PREFIX.size))
            await websocket.send(await stdout.readexactly(length))

    ws_task = asyncio.create_task(ws_to_child())
    child_task = asyncio.create_task(child_to_ws())
    try:
        # Stop as soon as *either* direction ends, so a clean close on one side
        # tears the child down promptly instead of blocking on the other pump.
        done, _ = await asyncio.wait({ws_task, child_task}, return_when=asyncio.FIRST_COMPLETED)
        # Surface an unexpected failure from whichever pump finished first; a
        # clean EOF (the child exited on close) or a hung-up peer is expected —
        # including the abrupt TCP shutdown the pool uses to end a single-use
        # worker (no WebSocket close frame), which surfaces as ConnectionClosed.
        for task in done:
            try:
                task.result()
            except (asyncio.IncompleteReadError, ConnectionError, ConnectionClosed):
                pass
    finally:
        # Cancel the still-running pump and reap both so neither is left as an
        # orphan task with an unretrieved exception, then tear the child down.
        for task in (ws_task, child_task):
            task.cancel()
        await asyncio.gather(ws_task, child_task, return_exceptions=True)
        if child.returncode is None:
            child.kill()
        await child.wait()


async def serve_relay(host: str, port: int, monty_bin: str) -> None:
    """Serves the relay until cancelled, printing the bound `ws://` URL once up."""

    async def handler(websocket: ServerConnection) -> None:
        await bridge_connection(websocket, monty_bin)

    # max_size=None: never reject a frame the monty protocol itself would accept.
    async with serve(handler, host, port, max_size=None) as server:
        bound_host, bound_port = server.sockets[0].getsockname()[:2]
        print(format_ws_url(bound_host, bound_port), flush=True)
        await asyncio.get_running_loop().create_future()  # run until cancelled


def format_ws_url(host: str, port: int) -> str:
    """Builds a dialable `ws://` URL from a bound `getsockname()` address.

    Handles the two address shapes a bind can produce that a naive
    `ws://{host}:{port}` gets wrong: IPv6 literals must be bracketed in a URI
    (`ws://[::1]:port`), and a wildcard bind (`0.0.0.0` / `::`) is not a usable
    destination, so it's mapped to the matching loopback address. A plain
    hostname is left untouched.
    """
    try:
        ip = ipaddress.ip_address(host)
    except ValueError:
        return f'ws://{host}:{port}'  # a hostname, not an IP literal
    if ip.is_unspecified:
        ip = ipaddress.ip_address('::1' if ip.version == 6 else '127.0.0.1')
    host_part = f'[{ip}]' if ip.version == 6 else str(ip)
    return f'ws://{host_part}:{port}'


def resolve_monty_bin(explicit: str | None) -> str:
    """The monty binary to spawn: explicit arg, then `$MONTY_BIN`, then `PATH`."""
    return explicit or os.environ.get('MONTY_BIN') or shutil.which('monty') or 'monty'


def main() -> None:
    parser = argparse.ArgumentParser(description='Bridge WebSocket connections to monty subprocess children.')
    parser.add_argument('--host', default='127.0.0.1')
    parser.add_argument('--port', type=int, default=8799, help='listen port (0 binds an ephemeral port)')
    parser.add_argument('--monty-bin', default=None, help='monty binary path (default: $MONTY_BIN, then PATH)')
    args = parser.parse_args()
    try:
        asyncio.run(serve_relay(args.host, args.port, resolve_monty_bin(args.monty_bin)))
    except KeyboardInterrupt:
        pass


if __name__ == '__main__':
    main()
