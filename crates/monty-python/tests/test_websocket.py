"""`AsyncMontyWebsocket` tests.

These drive the WebSocket transport end-to-end against the `scripts/websocket_relay.py`
relay, which bridges each WebSocket connection to a real `monty subprocess`
child (it only translates framing, so the actual protocol work is done by the
real binary). This is the same shape as a production relay, minus the remote
network hop — it exercises the dial, the WS send/recv path, inputs, and async
external-function suspension through the public Python class.
"""

from __future__ import annotations

import asyncio
import sys
from collections.abc import AsyncIterator
from pathlib import Path

import pytest
from inline_snapshot import snapshot

from pydantic_monty import AsyncMontyWebsocket, MontyRuntimeError
from pydantic_monty._binary import find_monty_binary

_RELAY_SCRIPT = Path(__file__).resolve().parents[3] / 'scripts' / 'websocket_relay.py'


@pytest.fixture
async def ws_url() -> AsyncIterator[str]:
    """Starts the relay on an ephemeral port and yields its `ws://` URL.

    Runs the standalone script with the test interpreter (so it picks up the
    `websockets` dev dependency) and reads back the URL it prints once listening.
    """
    relay = await asyncio.create_subprocess_exec(
        sys.executable,
        str(_RELAY_SCRIPT),
        '--port',
        '0',
        '--monty-bin',
        find_monty_binary(),
        stdout=asyncio.subprocess.PIPE,
    )
    assert relay.stdout is not None
    try:
        line = await asyncio.wait_for(relay.stdout.readline(), timeout=30)
        url = line.decode().strip()
        assert url.startswith('ws://'), f'relay did not announce a URL, got {url!r}'
        yield url
    finally:
        relay.terminate()
        await relay.wait()


async def test_feed_run_over_websocket(ws_url: str):
    async with AsyncMontyWebsocket(ws_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)
            # session state persists across feeds within a single checkout
            await session.feed_run('x = 21')
            assert await session.feed_run('x * 2') == snapshot(42)


async def test_inputs_and_async_external_function_over_websocket(ws_url: str):
    # exercises an external-function suspension being driven over the WebSocket
    async def double(x: int) -> int:
        return x * 2

    async with AsyncMontyWebsocket(ws_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            result = await session.feed_run(
                'await double(n) + 1',
                inputs={'n': 20},
                external_lookup={'double': double},
            )
    assert result == snapshot(41)


async def test_separate_checkouts_are_isolated(ws_url: str):
    # each checkout is a fresh single-use remote worker, so state must not leak
    async with AsyncMontyWebsocket(ws_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            await session.feed_run('leaked = 123')
        async with pool.checkout() as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run('leaked')
    assert exc_info.value.display(format='msg') == snapshot("name 'leaked' is not defined")
