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
import datetime
import importlib.util
import struct
import sys
import time
from collections.abc import AsyncIterator, Callable
from contextvars import ContextVar
from pathlib import Path
from types import MappingProxyType, ModuleType
from typing import Any

import pytest
from inline_snapshot import snapshot
from websockets.asyncio.server import ServerConnection, serve
from websockets.datastructures import Headers
from websockets.exceptions import ConnectionClosed
from websockets.http11 import Request

from pydantic_monty import AsyncMontyWebsocket, MontyRuntimeError, MontyShutdown
from pydantic_monty._binary import find_monty_binary

_RELAY_SCRIPT = Path(__file__).resolve().parents[3] / 'scripts' / 'websocket_relay.py'


@pytest.fixture
async def ws_url() -> AsyncIterator[str]:
    """Starts the relay script on an ephemeral port and yields its `ws://` URL.

    Runs the script as a subprocess, the way it ships, and reads back the URL
    it prints once listening.
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


@pytest.fixture
async def ws_url_capturing_headers() -> AsyncIterator[tuple[str, list[Headers]]]:
    """Serves the relay's bridge in-process on an ephemeral port and yields its
    `ws://` URL plus the headers of every upgrade request it receives.

    The same bridge as `ws_url` (each connection gets a real `monty subprocess`
    child), hosted here so `process_request` can record what each dial sent.
    """
    relay = _load_relay_script()
    monty_bin = find_monty_binary()
    captured: list[Headers] = []

    def capture(_connection: ServerConnection, request: Request) -> None:
        captured.append(request.headers)

    async def handler(websocket: ServerConnection) -> None:
        await relay.bridge_connection(websocket, monty_bin)

    async with serve(handler, '127.0.0.1', 0, process_request=capture, max_size=None) as server:
        host, port = server.sockets[0].getsockname()[:2]
        yield relay.format_ws_url(host, port), captured


@pytest.fixture
async def storing_ws_url() -> AsyncIterator[str]:
    """Serves a fake of a server that stores sessions, bridging each connection to a
    real `monty subprocess` child.

    It names the session on the reply to `Configure` and to `Load`, and the third
    request of the first connection is not run: the child is dumped, its state
    kept under the session's ID, and the request answered with `ShutdownDump`
    naming that ID. A later `Load` of the ID gives the child the stored state.
    """
    server = _StoringServer(find_monty_binary())
    async with serve(server.handle, '127.0.0.1', 0, max_size=None) as ws_server:
        host, port = ws_server.sockets[0].getsockname()[:2]
        yield _load_relay_script().format_ws_url(host, port)


_LENGTH_PREFIX = struct.Struct('<I')
# `ParentRequest.kind` and `ChildEvent.kind` field numbers, from `monty.proto`
_REQUEST_DUMP, _REQUEST_LOAD = 7, 8
_EVENT_PRINT, _EVENT_DUMP_RESULT, _EVENT_SHUTDOWN = 1, 9, 12
_EVENT_SESSION_ID = 28


class _StoringServer:
    """The fake behind `storing_ws_url`; see the fixture."""

    def __init__(self, monty_bin: str) -> None:
        self.monty_bin = monty_bin
        self.records: dict[bytes, bytes] = {}
        self.minted = 0
        self.connections = 0

    def _mint(self) -> bytes:
        self.minted += 1
        return f'sess-{self.minted}'.encode()

    async def handle(self, websocket: ServerConnection) -> None:
        self.connections += 1
        drain_at = 3 if self.connections == 1 else None
        child = await asyncio.create_subprocess_exec(
            self.monty_bin, 'subprocess', stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE
        )
        assert child.stdin is not None and child.stdout is not None
        stdin, stdout = child.stdin, child.stdout

        async def to_child(frame: bytes) -> None:
            stdin.write(_LENGTH_PREFIX.pack(len(frame)) + frame)
            await stdin.drain()

        async def from_child() -> bytes:
            (length,) = _LENGTH_PREFIX.unpack(await stdout.readexactly(_LENGTH_PREFIX.size))
            return await stdout.readexactly(length)

        session_id: bytes | None = None
        requests = 0
        try:
            async for message in websocket:
                assert isinstance(message, bytes)
                requests += 1
                fields = _proto_fields(message)
                if requests == drain_at:
                    # park: dump the idle child, keep its state under the ID,
                    # and tell the client the request did not run
                    assert session_id is not None
                    await to_child(_bytes_field(_REQUEST_DUMP, b''))
                    reply = _proto_fields(await from_child())
                    self.records[session_id] = _proto_fields(reply[_EVENT_DUMP_RESULT])[1]
                    await websocket.send(_bytes_field(_EVENT_SHUTDOWN, _bytes_field(1, session_id)))
                    return
                names = requests == 1 or _REQUEST_LOAD in fields
                if _REQUEST_LOAD in fields:
                    # the child loads the stored bytes the ID stands for
                    state = self.records[_proto_fields(fields[_REQUEST_LOAD])[1]]
                    message = _bytes_field(_REQUEST_LOAD, _bytes_field(1, state))
                await to_child(message)
                while True:
                    reply = await from_child()
                    kind = min(field for field in _proto_fields(reply) if field <= _EVENT_SHUTDOWN)
                    if kind != _EVENT_PRINT and names:
                        session_id = self._mint()
                        reply += _bytes_field(_EVENT_SESSION_ID, session_id)
                    await websocket.send(reply)
                    if kind != _EVENT_PRINT:
                        break
        except ConnectionClosed:
            pass
        finally:
            if child.returncode is None:
                child.kill()
            await child.wait()


def _proto_fields(buf: bytes) -> dict[int, Any]:
    """The top-level fields of one protobuf message, keyed by field number: an int
    for a varint, the bytes for a length-delimited field, and fixed-width values
    as raw bytes. Only what the fake needs, so a repeated field keeps its last value."""
    fields: dict[int, Any] = {}
    i = 0
    while i < len(buf):
        key, i = _varint(buf, i)
        field, wire_type = key >> 3, key & 7
        if wire_type == 0:
            fields[field], i = _varint(buf, i)
        elif wire_type == 2:
            length, i = _varint(buf, i)
            fields[field] = buf[i : i + length]
            i += length
        elif wire_type == 1:
            fields[field] = buf[i : i + 8]
            i += 8
        elif wire_type == 5:
            fields[field] = buf[i : i + 4]
            i += 4
        else:
            raise ValueError(f'unsupported wire type {wire_type}')
    return fields


def _varint(buf: bytes, i: int) -> tuple[int, int]:
    value = shift = 0
    while True:
        byte = buf[i]
        i += 1
        value |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return value, i
        shift += 7


def _encode_varint(value: int) -> bytes:
    out = bytearray()
    while value > 0x7F:
        out.append(value & 0x7F | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)


def _bytes_field(field: int, data: bytes) -> bytes:
    return _encode_varint(field << 3 | 2) + _encode_varint(len(data)) + data


def _load_relay_script() -> ModuleType:
    """Imports `scripts/websocket_relay.py`, which is a standalone script, not a package."""
    spec = importlib.util.spec_from_file_location('websocket_relay', _RELAY_SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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


async def test_os_policy_over_websocket(ws_url: str):
    # Options must survive serialization in `Configure`.
    frozen = datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)
    async with AsyncMontyWebsocket(ws_url, request_timeout=30.0) as pool:
        async with pool.checkout(
            os_policy={'datetime': frozen, 'sleep': 'zero', 'random_start': {'seed': 42}}
        ) as session:
            code = 'import random, time\nfrom datetime import datetime\ntime.sleep(3600)\n(datetime.now(), random.random())'
            started = time.monotonic()
            assert await session.feed_run(code) == snapshot(
                (datetime.datetime(2024, 1, 15, 10, 30, 5, 123456), 0.6394267984578837)
            )
            assert time.monotonic() - started < 5


async def test_separate_checkouts_are_isolated(ws_url: str):
    # each checkout is a fresh single-use remote worker, so state must not leak
    async with AsyncMontyWebsocket(ws_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            await session.feed_run('leaked = 123')
        async with pool.checkout() as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run('leaked')
    assert exc_info.value.display(format='msg') == snapshot("name 'leaked' is not defined")


async def test_connect_headers_sent_per_checkout(ws_url_capturing_headers: tuple[str, list[Headers]]):
    """`connect_headers` is called once per checkout, on the checking-out task,
    so it sees that task's contextvars — the `traceparent` use case."""
    url, captured = ws_url_capturing_headers
    trace_id: ContextVar[str] = ContextVar('trace_id')
    calls = 0

    def connect_headers() -> dict[str, str]:
        nonlocal calls
        calls += 1
        return {'traceparent': trace_id.get()}

    async def checkout_as(trace: str) -> int:
        # a task's contextvar assignment is invisible to its siblings, so distinct
        # traceparents on the two dials prove the callback ran in each checkout's own task
        trace_id.set(trace)
        async with pool.checkout() as session:
            return await session.feed_run('1 + 1')

    async with AsyncMontyWebsocket(url, request_timeout=30.0, connect_headers=connect_headers) as pool:
        results = await asyncio.gather(checkout_as('00-aaa-111-01'), checkout_as('00-bbb-222-01'))
    assert results == snapshot([2, 2])
    assert calls == snapshot(2)
    assert sorted(headers['traceparent'] for headers in captured) == snapshot(['00-aaa-111-01', '00-bbb-222-01'])


async def test_connect_headers_accepts_any_mapping(ws_url_capturing_headers: tuple[str, list[Headers]]):
    """Any `Mapping` is accepted, not just `dict`."""
    url, captured = ws_url_capturing_headers

    def connect_headers() -> MappingProxyType[str, str]:
        return MappingProxyType({'x-token': 't'})

    async with AsyncMontyWebsocket(url, request_timeout=30.0, connect_headers=connect_headers) as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)
    (upgrade_headers,) = captured
    assert upgrade_headers.get('x-token') == snapshot('t')


def test_connect_headers_not_callable():
    not_callable: Any = {'x-token': 't'}
    with pytest.raises(TypeError) as exc_info:
        AsyncMontyWebsocket('ws://127.0.0.1:9', connect_headers=not_callable)
    assert exc_info.value.args[0] == snapshot("'dict' object is not callable")


async def test_connect_headers_failure_leaves_the_pool_usable(
    ws_url_capturing_headers: tuple[str, list[Headers]],
):
    """A failing callback checks nothing out, so the next checkout works normally."""
    url, captured = ws_url_capturing_headers
    attempts = 0

    def connect_headers() -> dict[str, str]:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise ValueError('no token yet')
        return {'x-token': 't'}

    async with AsyncMontyWebsocket(url, request_timeout=30.0, connect_headers=connect_headers) as pool:
        with pytest.raises(ValueError) as exc_info:
            async with pool.checkout():
                pass
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)
    assert exc_info.value.args[0] == snapshot('no token yet')
    assert [headers.get('x-token') for headers in captured] == snapshot(['t'])


async def test_connect_headers_not_called_on_an_inactive_pool():
    """The pool is checked before the callback runs, so a pool that was never
    entered raises without calling it."""
    calls = 0

    def connect_headers() -> dict[str, str]:
        nonlocal calls
        calls += 1
        return {}

    pool = AsyncMontyWebsocket('ws://127.0.0.1:9', connect_headers=connect_headers)
    with pytest.raises(RuntimeError) as exc_info:
        async with pool.checkout():
            pass
    assert exc_info.value.args[0] == snapshot(
        'the pool is not active — enter the Monty / AsyncMonty context manager first'
    )
    assert calls == snapshot(0)


def raise_no_token() -> dict[str, str]:
    raise ValueError('no token available')


@pytest.mark.parametrize(
    'connect_headers, exc_type, message',
    [
        (
            lambda: [('x-token', 't')],
            TypeError,
            snapshot("connect_headers must return a mapping of str to str, got 'list'"),
        ),
        (
            lambda: {1: 't'},
            TypeError,
            snapshot("connect_headers must return a mapping of str to str, got 'int' header name"),
        ),
        (
            lambda: {'x-token': 1},
            TypeError,
            snapshot("connect_headers must return a mapping of str to str, got 'int' header value"),
        ),
        (
            lambda: {'bad header': 't'},
            RuntimeError,
            snapshot(
                'failed to spawn monty worker: ws://127.0.0.1:9: connect header "bad header": invalid HTTP header name'
            ),
        ),
        (
            lambda: {'x-token': 'a\nb'},
            RuntimeError,
            snapshot(
                'failed to spawn monty worker: ws://127.0.0.1:9: connect header "x-token" value: failed to parse header value'
            ),
        ),
        (raise_no_token, ValueError, snapshot('no token available')),
        # a `str` that is not encodable — a lone surrogate — cannot become a header
        (
            lambda: {'x-token': '\udc80'},
            UnicodeEncodeError,
            snapshot("'utf-8' codec can't encode character '\\udc80' in position 0: surrogates not allowed"),
        ),
    ],
)
async def test_connect_headers_errors_raise_on_entry(
    connect_headers: Callable[[], Any], exc_type: type[Exception], message: str
):
    """The callback runs and its result is checked as the session is entered,
    before any dial, so an unreachable URL is fine."""
    async with AsyncMontyWebsocket('ws://127.0.0.1:9', connect_headers=connect_headers) as pool:
        with pytest.raises(exc_type) as exc_info:
            async with pool.checkout():
                pass
    assert str(exc_info.value) == message


async def test_checkout_rejects_unknown_limits():
    """`checkout()` validates its arguments up front, before any dial, like `Monty.checkout`."""
    async with AsyncMontyWebsocket('ws://127.0.0.1:9') as pool:
        with pytest.raises(ValueError) as exc_info:
            pool.checkout(limits={'max_memroy': 10_000_000})  # pyright: ignore[reportArgumentType]
    assert exc_info.value.args[0] == snapshot(
        "unknown limits key 'max_memroy'; accepted keys are 'max_feed_duration_secs', 'max_turn_duration_secs', 'max_memory', 'gc_interval', 'max_recursion_depth', 'max_suspensions', 'max_total_sleep_secs'"
    )


async def test_plain_relay_names_no_session(ws_url: str):
    """The dev relay stores nothing, so sessions get no ID and `ephemeral` changes nothing."""
    async with AsyncMontyWebsocket(ws_url, request_timeout=30.0) as pool:
        for ephemeral in (None, True, False):
            async with pool.checkout(ephemeral=ephemeral) as session:
                assert session.session_id is None
                assert await session.feed_run('1 + 1') == snapshot(2)


async def test_loaded_session_has_no_id_on_a_plain_relay(ws_url: str):
    """Without storage `state` is dump bytes, and the loaded session gets no ID."""
    async with AsyncMontyWebsocket(ws_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            await session.feed_run('x = 20')
            dump = await session.dump()
        async with pool.checkout() as session:
            await session.load_session(dump)
            assert session.session_id is None
            assert await session.feed_run('x + 1') == snapshot(21)


async def test_auto_resume_can_be_disabled(ws_url: str):
    """Only a storing server's drain triggers a resume, so a plain relay behaves the same either way."""
    async with AsyncMontyWebsocket(ws_url, request_timeout=30.0, auto_resume=False) as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)


async def test_storing_server_names_the_session_and_resumes_a_shutdown(storing_ws_url: str):
    async with AsyncMontyWebsocket(storing_ws_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            assert session.session_id == snapshot(b'sess-1')
            await session.feed_run('x = 20')
            # the fake parks the session instead of running this feed; the
            # client reloads the ID on a new connection and re-sends it
            assert await session.feed_run('x + 1') == snapshot(21)
        # the ID the resume adopted outlives the checkout, even when nothing
        # read it while the checkout was live
        assert session.session_id == snapshot(b'sess-3')


async def test_auto_resume_disabled_raises_shutdown_naming_the_session(storing_ws_url: str):
    async with AsyncMontyWebsocket(storing_ws_url, request_timeout=30.0, auto_resume=False) as pool:
        async with pool.checkout() as session:
            await session.feed_run('x = 20')
            with pytest.raises(MontyShutdown) as exc_info:
                await session.feed_run('x + 1')
            assert exc_info.value.dump == snapshot(b'sess-1')
        async with pool.checkout() as session:
            await session.load_session(b'sess-1')
            assert session.session_id == snapshot(b'sess-3')
            assert await session.feed_run('x + 1') == snapshot(21)
