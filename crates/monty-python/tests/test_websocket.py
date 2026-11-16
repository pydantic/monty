"""Tests for `AsyncMontyRemote` over WebSocket (`ws://` URLs).

A fake server bridges each connection to a real `monty subprocess` worker:
one binary message per protocol frame, length-prefixed on the worker's stdio.
Variants refuse the upgrade or close the connection with a policy code, the
way `monty-server` reports a session it will not run or has ended.
"""

from __future__ import annotations

import asyncio
import struct
from collections.abc import AsyncGenerator, Awaitable, Callable
from contextlib import asynccontextmanager
from http import HTTPStatus

import pytest
from inline_snapshot import snapshot
from websockets.asyncio.server import ServerConnection, serve
from websockets.http11 import Request, Response

from pydantic_monty import AsyncMontyRemote, MontyConnectError, MontyDisconnectError, MontyRefusedError
from pydantic_monty._binary import find_monty_binary

# the worker's per-frame header on stdio: a 4-byte little-endian length
_LENGTH_PREFIX = struct.Struct('<I')

# `Cause::TurnTimeout.close_code()`: a server's close code for a turn over its cap
_TURN_TIMEOUT = 4002

Handler = Callable[[ServerConnection], Awaitable[None]]
ProcessRequest = Callable[[ServerConnection, Request], Response | None]


@asynccontextmanager
async def _serve(handler: Handler, process_request: ProcessRequest | None = None) -> AsyncGenerator[str]:
    """Serves `handler` on an ephemeral port and yields its `ws://` URL."""
    async with serve(handler, '127.0.0.1', 0, process_request=process_request, max_size=None) as server:
        port = next(iter(server.sockets)).getsockname()[1]
        yield f'ws://127.0.0.1:{port}'


async def _bridge(connection: ServerConnection, first: str | bytes | None = None) -> None:
    """Bridges one connection to a fresh `monty subprocess` worker until either side ends,
    starting with `first` when the caller already read the connection's first message."""
    child = await asyncio.create_subprocess_exec(
        find_monty_binary(), 'subprocess', stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE
    )
    assert child.stdin is not None and child.stdout is not None
    stdin, stdout = child.stdin, child.stdout

    async def to_child() -> None:
        if first is not None:
            assert isinstance(first, bytes)
            stdin.write(_LENGTH_PREFIX.pack(len(first)) + first)
            await stdin.drain()
        async for message in connection:
            assert isinstance(message, bytes)
            stdin.write(_LENGTH_PREFIX.pack(len(message)) + message)
            await stdin.drain()
        stdin.close()

    async def from_child() -> None:
        try:
            while True:
                (length,) = _LENGTH_PREFIX.unpack(await stdout.readexactly(_LENGTH_PREFIX.size))
                await connection.send(await stdout.readexactly(length))
        except asyncio.IncompleteReadError:
            pass

    pumps = [asyncio.create_task(to_child()), asyncio.create_task(from_child())]
    try:
        await asyncio.wait(pumps, return_when=asyncio.FIRST_COMPLETED)
    finally:
        for pump in pumps:
            pump.cancel()
        await asyncio.gather(*pumps, return_exceptions=True)
        if child.returncode is None:
            child.kill()
        await child.wait()


async def test_feed_run_over_websocket():
    seen: list[tuple[str, str | None]] = []

    async def handler(connection: ServerConnection) -> None:
        request = connection.request
        assert request is not None
        seen.append((request.path, request.headers.get('authorization')))
        await _bridge(connection)

    async with _serve(handler) as url:
        async with AsyncMontyRemote(
            f'{url}/relay?tenant=a', request_timeout=30.0, metadata=lambda: {'authorization': 'Bearer t0k3n'}
        ) as pool:
            async with pool.checkout() as session:
                assert await session.feed_run('1 + 1') == snapshot(2)
                await session.feed_run('x = 21')
                assert await session.feed_run('x * 2') == snapshot(42)
    # dialed as given: a relay may route on the path and query
    assert seen == snapshot([('/relay?tenant=a', 'Bearer t0k3n')])


async def test_a_refused_upgrade_raises_a_refusal():
    def refuse(connection: ServerConnection, request: Request) -> Response:
        response = connection.respond(HTTPStatus.SERVICE_UNAVAILABLE, 'at capacity')
        response.headers['monty-cause'] = 'capacity'
        response.headers['retry-after'] = '3'
        return response

    async with _serve(_bridge, refuse) as url:
        async with AsyncMontyRemote(url) as pool:
            with pytest.raises(MontyRefusedError) as exc_info:
                async with pool.checkout():
                    pass
    err = exc_info.value
    assert (err.status_code, err.cause, err.status_message, err.retry_after, err.retryable) == snapshot(
        ('RESOURCE_EXHAUSTED', 'capacity', 'HTTP 503 Service Unavailable: at capacity', 3.0, True)
    )


async def test_a_not_found_upgrade_is_a_connect_failure():
    def not_found(connection: ServerConnection, request: Request) -> Response:
        return connection.respond(HTTPStatus.NOT_FOUND, 'no such route')

    async with _serve(_bridge, not_found) as url:
        async with AsyncMontyRemote(url) as pool:
            with pytest.raises(MontyConnectError) as exc_info:
                async with pool.checkout():
                    pass
    assert str(exc_info.value).removeprefix(url) == snapshot(
        ': the server refused the WebSocket upgrade: HTTP 404 Not Found: no such route'
    )


async def test_a_policy_close_names_its_cause():
    """A server ending the session by policy closes with the cause's own code,
    which the error reports without parsing the reason text."""

    async def handler(connection: ServerConnection) -> None:
        # the session's first message carries its configuration and first request
        await connection.recv()
        await connection.close(_TURN_TIMEOUT, 'turn exceeded 30s')

    async with _serve(handler) as url:
        async with AsyncMontyRemote(url, auto_resume=False) as pool:
            async with pool.checkout() as session:
                with pytest.raises(MontyDisconnectError) as exc_info:
                    await session.feed_run('while True: pass')
    err = exc_info.value
    assert (err.status_code, err.cause, err.status_message, err.transport_error) == snapshot(
        ('DEADLINE_EXCEEDED', 'turn_timeout', 'turn exceeded 30s', None)
    )
    assert str(err) == snapshot(
        'monty worker connection closed while waiting for a reply: DEADLINE_EXCEEDED (turn_timeout): turn exceeded 30s'
    )


async def _wait_for(condition: Callable[[], bool]) -> None:
    """Polls `condition` for up to five seconds."""
    for _ in range(500):
        if condition():
            return
        await asyncio.sleep(0.01)
    raise AssertionError('condition not met within 5s')


async def test_pre_warm_opens_sessions_before_checkout():
    """`__aenter__` opens the pre-warmed sessions; a checkout takes one, and it is replaced."""
    connections: list[ServerConnection] = []
    used: list[ServerConnection] = []

    async def handler(connection: ServerConnection) -> None:
        connections.append(connection)
        first = await connection.recv()
        used.append(connection)
        await _bridge(connection, first)

    async with _serve(handler) as url:
        async with AsyncMontyRemote(url, request_timeout=30.0, pre_warm=2) as pool:
            await _wait_for(lambda: len(connections) == 2)
            async with pool.checkout() as session:
                assert await session.feed_run('1 + 1') == snapshot(2)
            await _wait_for(lambda: len(connections) == 3)
            assert len(used) == 1
            assert used[0] in connections[:2]


async def test_a_checkout_waits_for_a_pre_warmed_session_still_opening():
    """Without `pre_warm_wait`, checking out straight after `__aenter__` takes the session already dialing,
    rather than dialing another."""
    connections: list[ServerConnection] = []
    used: list[ServerConnection] = []

    async def handler(connection: ServerConnection) -> None:
        connections.append(connection)
        first = await connection.recv()
        used.append(connection)
        await _bridge(connection, first)

    async with _serve(handler) as url:
        async with AsyncMontyRemote(url, request_timeout=30.0, pre_warm=1, pre_warm_wait=False) as pool:
            async with pool.checkout() as session:
                assert await session.feed_run('1 + 1') == snapshot(2)
            await _wait_for(lambda: len(connections) == 2)
            assert used == [connections[0]]


async def test_pre_warm_skips_a_session_the_server_closed():
    connections: list[ServerConnection] = []

    async def handler(connection: ServerConnection) -> None:
        connections.append(connection)
        if len(connections) == 1:
            await connection.close()
        else:
            await _bridge(connection)

    async with _serve(handler) as url:
        async with AsyncMontyRemote(url, request_timeout=30.0, pre_warm=1) as pool:
            await _wait_for(lambda: len(connections) == 1 and connections[0].state.name == 'CLOSED')
            await asyncio.sleep(0.05)
            async with pool.checkout() as session:
                assert await session.feed_run('1 + 1') == snapshot(2)


def test_pre_warm_cannot_take_metadata():
    with pytest.raises(ValueError) as exc_info:
        AsyncMontyRemote('ws://127.0.0.1:1', pre_warm=1, metadata=lambda: {})
    assert exc_info.value.args[0] == snapshot(
        'pre_warm cannot be combined with metadata: pre-warmed sessions open before any checkout, without it'
    )


async def test_pre_warm_wait_raises_from_aenter_when_a_session_cannot_open():
    def not_found(connection: ServerConnection, request: Request) -> Response:
        return connection.respond(HTTPStatus.NOT_FOUND, 'no such route')

    async with _serve(_bridge, not_found) as url:
        with pytest.raises(MontyConnectError) as exc_info:
            async with AsyncMontyRemote(url, pre_warm=1):
                pass
    assert str(exc_info.value).removeprefix(url) == snapshot(
        ': the server refused the WebSocket upgrade: HTTP 404 Not Found: no such route'
    )
