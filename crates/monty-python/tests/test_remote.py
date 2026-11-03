"""`AsyncMontyRemote` tests.

These drive the remote transport end-to-end against the `scripts/grpc_relay.py`
relay, which bridges each session stream to a real `monty subprocess` child
(it only translates framing, so the actual protocol work is done by the real
binary). This is the same shape as a production relay, minus the remote
network hop — it exercises the dial, the gRPC send/recv path, inputs, and async
external-function suspension through the public Python class.
"""

from __future__ import annotations

import asyncio
import datetime
import importlib.util
import struct
import sys
import time
from collections.abc import AsyncGenerator, AsyncIterator, Callable
from contextlib import asynccontextmanager
from contextvars import ContextVar
from pathlib import Path
from types import MappingProxyType, ModuleType
from typing import Any

import grpc
import pytest
from grpc import aio
from inline_snapshot import snapshot

from pydantic_monty import (
    AsyncMontyRemote,
    AsyncMontyWebsocket,
    ExternalModule,
    MontyDisconnectError,
    MontyRuntimeError,
    MontyShutdown,
)
from pydantic_monty._binary import find_monty_binary

_RELAY_SCRIPT = Path(__file__).resolve().parents[3] / 'scripts' / 'grpc_relay.py'


@pytest.fixture
async def relay_url() -> AsyncIterator[str]:
    """Starts the relay script on an ephemeral port and yields its `http://` URL.

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
        assert url.startswith('http://'), f'relay did not announce a URL, got {url!r}'
        yield url
    finally:
        relay.terminate()
        await relay.wait()


@pytest.fixture
async def relay_url_capturing_metadata() -> AsyncIterator[tuple[str, list[dict[str, str]]]]:
    """Serves the relay's bridge in-process on an ephemeral port and yields its
    `http://` URL plus the request metadata of every stream it receives.

    The same bridge as `relay_url` (each stream gets a real `monty subprocess`
    child), hosted here so the behaviour can record what each dial sent.
    """
    relay = _load_relay_script()
    monty_bin = find_monty_binary()
    captured: list[dict[str, str]] = []

    async def behaviour(
        requests: AsyncIterator[bytes], context: aio.ServicerContext[bytes, bytes]
    ) -> AsyncIterator[bytes]:
        captured.append({key: value for key, value in context.invocation_metadata() or () if isinstance(value, str)})
        await relay.admit(context)
        async for event in relay.bridge_stream(requests, monty_bin):
            yield event

    server, url = await relay.start_server(relay.SessionHandler(behaviour), '127.0.0.1', 0)
    try:
        yield url, captured
    finally:
        await server.stop(None)


@pytest.fixture
async def storing_url() -> AsyncIterator[str]:
    """Serves a fake of a server that stores sessions, bridging each stream to a
    real `monty subprocess` child.

    It names the session on the reply to `Configure` and to `Load`, and the third
    request of the first stream is not run: the child is dumped, its state kept
    under the session's ID, and the request answered with `ShutdownDump` naming
    that ID. A later `Load` of the ID gives the child the stored state.
    """
    relay = _load_relay_script()
    server, url = await relay.start_server(
        relay.SessionHandler(_StoringServer(find_monty_binary()).handle), '127.0.0.1', 0
    )
    try:
        yield url
    finally:
        await server.stop(None)


_LENGTH_PREFIX = struct.Struct('<I')
# `ParentRequest.kind` and `ChildEvent.kind` field numbers, from `monty.proto`
_REQUEST_DUMP, _REQUEST_LOAD = 7, 8
_EVENT_PRINT, _EVENT_DUMP_RESULT, _EVENT_OK, _EVENT_SHUTDOWN = 1, 9, 10, 12
# `ChildEvent.kind` arms own the tags below this; the message-level fields start here
_FIRST_EVENT_FIELD = 20
_EVENT_SESSION_ID = 28


class _StoringServer:
    """The fake behind `storing_url`; see the fixture."""

    def __init__(self, monty_bin: str) -> None:
        self.monty_bin = monty_bin
        self.records: dict[bytes, bytes] = {}
        self.minted = 0
        self.streams = 0

    def _mint(self) -> bytes:
        self.minted += 1
        return f'sess-{self.minted}'.encode()

    async def handle(
        self, requests: AsyncIterator[bytes], context: aio.ServicerContext[bytes, bytes]
    ) -> AsyncIterator[bytes]:
        await context.send_initial_metadata(())  # admit the stream; see `admit` in the relay script
        self.streams += 1
        drain_at = 3 if self.streams == 1 else None
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
        request_count = 0
        try:
            async for message in requests:
                request_count += 1
                fields = _proto_fields(message)
                if request_count == drain_at:
                    # park: dump the idle child, keep its state under the ID,
                    # and tell the client the request did not run
                    assert session_id is not None
                    await to_child(_bytes_field(_REQUEST_DUMP, b''))
                    reply = _proto_fields(await from_child())
                    self.records[session_id] = _proto_fields(reply[_EVENT_DUMP_RESULT])[1]
                    yield _bytes_field(_EVENT_SHUTDOWN, _bytes_field(1, session_id))
                    return
                names = request_count == 1 or _REQUEST_LOAD in fields
                if _REQUEST_LOAD in fields:
                    # the child loads the stored bytes the ID stands for
                    state = self.records[_proto_fields(fields[_REQUEST_LOAD])[1]]
                    message = _bytes_field(_REQUEST_LOAD, _bytes_field(1, state))
                await to_child(message)
                while True:
                    reply = await from_child()
                    kind = min(field for field in _proto_fields(reply) if field < _FIRST_EVENT_FIELD)
                    if kind != _EVENT_PRINT and names:
                        session_id = self._mint()
                        reply += _bytes_field(_EVENT_SESSION_ID, session_id)
                    yield reply
                    if kind != _EVENT_PRINT:
                        break
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
    """Imports `scripts/grpc_relay.py`, which is a standalone script, not a package."""
    spec = importlib.util.spec_from_file_location('grpc_relay', _RELAY_SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


async def test_feed_run_over_grpc(relay_url: str):
    async with AsyncMontyRemote(relay_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)
            # session state persists across feeds within a single checkout
            await session.feed_run('x = 21')
            assert await session.feed_run('x * 2') == snapshot(42)


def test_websocket_name_is_an_alias():
    """The former name still resolves to the same class."""
    assert AsyncMontyWebsocket is AsyncMontyRemote


async def test_websocket_urls_are_accepted(relay_url: str):
    """A `ws://` URL kept from the WebSocket days dials the same server over gRPC."""
    async with AsyncMontyWebsocket('ws://' + relay_url.removeprefix('http://'), request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)


async def test_inputs_and_async_external_function_over_grpc(relay_url: str):
    # exercises an external-function suspension being driven over the stream
    async def double(x: int) -> int:
        return x * 2

    async with AsyncMontyRemote(relay_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            result = await session.feed_run(
                'await double(n) + 1',
                inputs={'n': 20},
                external_lookup={'double': double},
            )
    assert result == snapshot(41)


async def test_os_policy_over_grpc(relay_url: str):
    # Options must survive serialization in `Configure`.
    frozen = datetime.datetime(2024, 1, 15, 10, 30, 5, 123456)
    async with AsyncMontyRemote(relay_url, request_timeout=30.0) as pool:
        async with pool.checkout(
            os_policy={'datetime': frozen, 'sleep': 'zero', 'random_start': {'seed': 42}}
        ) as session:
            code = 'import random, time\nfrom datetime import datetime\ntime.sleep(3600)\n(datetime.now(), random.random())'
            started = time.monotonic()
            assert await session.feed_run(code) == snapshot(
                (datetime.datetime(2024, 1, 15, 10, 30, 5, 123456), 0.6394267984578837)
            )
            assert time.monotonic() - started < 5


async def test_separate_checkouts_are_isolated(relay_url: str):
    # each checkout is a fresh single-use remote worker, so state must not leak
    async with AsyncMontyRemote(relay_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            await session.feed_run('leaked = 123')
        async with pool.checkout() as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run('leaked')
    assert exc_info.value.display(format='msg') == snapshot("name 'leaked' is not defined")


async def test_connect_headers_sent_per_checkout(relay_url_capturing_metadata: tuple[str, list[dict[str, str]]]):
    """`connect_headers` is called once per checkout, on the checking-out task,
    so it sees that task's contextvars — the `traceparent` use case."""
    url, captured = relay_url_capturing_metadata
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

    async with AsyncMontyRemote(url, request_timeout=30.0, connect_headers=connect_headers) as pool:
        results = await asyncio.gather(checkout_as('00-aaa-111-01'), checkout_as('00-bbb-222-01'))
    assert results == snapshot([2, 2])
    assert calls == snapshot(2)
    assert sorted(metadata['traceparent'] for metadata in captured) == snapshot(['00-aaa-111-01', '00-bbb-222-01'])


async def test_connect_headers_accepts_any_mapping(relay_url_capturing_metadata: tuple[str, list[dict[str, str]]]):
    """Any `Mapping` is accepted, not just `dict`."""
    url, captured = relay_url_capturing_metadata

    def connect_headers() -> MappingProxyType[str, str]:
        return MappingProxyType({'x-token': 't'})

    async with AsyncMontyRemote(url, request_timeout=30.0, connect_headers=connect_headers) as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)
    (metadata,) = captured
    assert metadata.get('x-token') == snapshot('t')
    # the pool names itself on every stream, ahead of tonic's own version
    assert metadata['user-agent'].startswith('monty-pool/')


def test_connect_headers_not_callable():
    not_callable: Any = {'x-token': 't'}
    with pytest.raises(TypeError) as exc_info:
        AsyncMontyRemote('http://127.0.0.1:9', connect_headers=not_callable)
    assert exc_info.value.args[0] == snapshot("'dict' object is not callable")


async def test_connect_headers_failure_leaves_the_pool_usable(
    relay_url_capturing_metadata: tuple[str, list[dict[str, str]]],
):
    """A failing callback checks nothing out, so the next checkout works normally."""
    url, captured = relay_url_capturing_metadata
    attempts = 0

    def connect_headers() -> dict[str, str]:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise ValueError('no token yet')
        return {'x-token': 't'}

    async with AsyncMontyRemote(url, request_timeout=30.0, connect_headers=connect_headers) as pool:
        with pytest.raises(ValueError) as exc_info:
            async with pool.checkout():
                pass
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)
    assert exc_info.value.args[0] == snapshot('no token yet')
    assert [metadata.get('x-token') for metadata in captured] == snapshot(['t'])


async def test_connect_headers_not_called_on_an_inactive_pool():
    """The pool is checked before the callback runs, so a pool that was never
    entered raises without calling it."""
    calls = 0

    def connect_headers() -> dict[str, str]:
        nonlocal calls
        calls += 1
        return {}

    pool = AsyncMontyRemote('http://127.0.0.1:9', connect_headers=connect_headers)
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
                'failed to spawn monty worker: http://127.0.0.1:9: connect header "bad header": invalid gRPC metadata key name'
            ),
        ),
        (
            lambda: {'x-token': 'a\nb'},
            RuntimeError,
            snapshot(
                'failed to spawn monty worker: http://127.0.0.1:9: connect header "x-token" value: failed to parse metadata value'
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
    async with AsyncMontyRemote('http://127.0.0.1:9', connect_headers=connect_headers) as pool:
        with pytest.raises(exc_type) as exc_info:
            async with pool.checkout():
                pass
    assert str(exc_info.value) == message


async def test_checkout_rejects_unknown_limits():
    """`checkout()` validates its arguments up front, before any dial, like `Monty.checkout`."""
    async with AsyncMontyRemote('http://127.0.0.1:9') as pool:
        with pytest.raises(ValueError) as exc_info:
            pool.checkout(limits={'max_memroy': 10_000_000})  # pyright: ignore[reportArgumentType]
    assert exc_info.value.args[0] == snapshot(
        "unknown limits key 'max_memroy'; accepted keys are 'max_feed_duration_secs', 'max_turn_duration_secs', 'max_memory', 'gc_interval', 'max_recursion_depth', 'max_suspensions', 'max_total_sleep_secs'"
    )


async def test_unreachable_server_fails_the_checkout():
    """Nothing is dialed until a checkout, which then reports the dial failure."""
    async with AsyncMontyRemote('http://127.0.0.1:9') as pool:
        with pytest.raises(RuntimeError) as exc_info:
            async with pool.checkout():
                pass
    assert exc_info.value.args[0].startswith('failed to spawn monty worker: http://127.0.0.1:9: ')


async def test_plain_relay_names_no_session(relay_url: str):
    """The dev relay stores nothing, so sessions get no ID and `ephemeral` changes nothing."""
    async with AsyncMontyRemote(relay_url, request_timeout=30.0) as pool:
        for ephemeral in (None, True, False):
            async with pool.checkout(ephemeral=ephemeral) as session:
                assert session.session_id is None
                assert await session.feed_run('1 + 1') == snapshot(2)


async def test_plain_relay_ignores_profile(relay_url: str):
    """The dev relay forwards `profile` to its worker, which ignores it."""
    async with AsyncMontyRemote(relay_url, request_timeout=30.0) as pool:
        for profile in (None, 'gpu', ''):
            async with pool.checkout(profile=profile) as session:
                assert await session.feed_run('1 + 1') == snapshot(2)


async def test_loaded_session_has_no_id_on_a_plain_relay(relay_url: str):
    """Without storage `state` is dump bytes, and the loaded session gets no ID."""
    async with AsyncMontyRemote(relay_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            await session.feed_run('x = 20')
            dump = await session.dump()
        async with pool.checkout() as session:
            await session.load_session(dump)
            assert session.session_id is None
            assert await session.feed_run('x + 1') == snapshot(21)


async def test_auto_resume_can_be_disabled(relay_url: str):
    """Only a storing server's drain triggers a resume, so a plain relay behaves the same either way."""
    async with AsyncMontyRemote(relay_url, request_timeout=30.0, auto_resume=False) as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == snapshot(2)


async def test_storing_server_names_the_session_and_resumes_a_shutdown(storing_url: str):
    async with AsyncMontyRemote(storing_url, request_timeout=30.0) as pool:
        async with pool.checkout() as session:
            assert session.session_id == snapshot(b'sess-1')
            await session.feed_run('x = 20')
            # the fake parks the session instead of running this feed; the
            # client reloads the ID on a new stream and re-sends it
            assert await session.feed_run('x + 1') == snapshot(21)
        # the ID the resume adopted outlives the checkout, even when nothing
        # read it while the checkout was live
        assert session.session_id == snapshot(b'sess-3')


async def test_auto_resume_disabled_raises_shutdown_naming_the_session(storing_url: str):
    async with AsyncMontyRemote(storing_url, request_timeout=30.0, auto_resume=False) as pool:
        async with pool.checkout() as session:
            await session.feed_run('x = 20')
            with pytest.raises(MontyShutdown) as exc_info:
                await session.feed_run('x + 1')
            assert exc_info.value.dump == snapshot(b'sess-1')
        async with pool.checkout() as session:
            await session.load_session(b'sess-1')
            assert session.session_id == snapshot(b'sess-3')
            assert await session.feed_run('x + 1') == snapshot(21)


async def test_module_stubs_over_grpc(relay_url: str):
    def add(a: int, b: int) -> int:
        return a + b

    modules = {'tools': ExternalModule({'add': add}, stubs='def add(a: int, b: int) -> int: ...\n')}
    async with AsyncMontyRemote(relay_url) as pool:
        async with pool.checkout(type_check=True, external_modules=modules) as session:
            assert await session.feed_run('import tools\ntools.add(1, 2)') == snapshot(3)


async def test_host_decode_budget_rejects_large_results(relay_url: str):
    async with AsyncMontyRemote(relay_url, max_decode_bytes=4096) as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('6 * 7') == snapshot(42)
            with pytest.raises(RuntimeError, match='frame exceeds decode memory budget'):
                await session.feed_run('[0] * 100_000')
        async with pool.checkout() as session:
            assert await session.feed_run('6 * 7') == snapshot(42)


def test_decode_budget_preserves_timeout_validation():
    with pytest.raises(ValueError, match='invalid request_timeout:'):
        AsyncMontyRemote('http://127.0.0.1:1', max_decode_bytes=4096, request_timeout=-1.0)


@pytest.mark.parametrize('max_decode_bytes', [None, 0, 1, (1 << 30) - 1, 1 << 30])
def test_decode_budget_accepts_values_within_ceiling(max_decode_bytes: int | None):
    AsyncMontyRemote('http://127.0.0.1:1', max_decode_bytes=max_decode_bytes)


@pytest.mark.parametrize('max_decode_bytes', [(1 << 30) + 1, sys.maxsize])
def test_decode_budget_rejects_values_above_ceiling(max_decode_bytes: int):
    with pytest.raises(ValueError, match=r'^max_decode_bytes must be at most 1073741824 bytes \(1 GiB\)$'):
        AsyncMontyRemote('http://127.0.0.1:1', max_decode_bytes=max_decode_bytes)


@pytest.mark.parametrize('max_decode_bytes', [-1, 1 << 128])
def test_decode_budget_rejects_values_outside_usize(max_decode_bytes: int):
    with pytest.raises(OverflowError):
        AsyncMontyRemote('http://127.0.0.1:1', max_decode_bytes=max_decode_bytes)


@asynccontextmanager
async def _serve_behaviour(behaviour: Any) -> AsyncGenerator[str]:
    """Serves `behaviour` as the session RPC for the block, yielding the URL to dial."""
    relay = _load_relay_script()
    server, url = await relay.start_server(relay.SessionHandler(behaviour), '127.0.0.1', 0)
    try:
        yield url
    finally:
        await server.stop(None)


@pytest.mark.parametrize(
    'status,message,cause',
    [
        (grpc.StatusCode.RESOURCE_EXHAUSTED, 'session capacity exhausted', 'capacity'),
        (grpc.StatusCode.DEADLINE_EXCEEDED, '再起動中 🐍', 'idle_timeout'),
        (grpc.StatusCode.INTERNAL, 'relay fell over', None),
        (None, None, None),
    ],
)
async def test_disconnect_details(status: grpc.StatusCode | None, message: str | None, cause: str | None):
    """A server's status, with or without a cause, and a plain stream end keep
    distinct diagnostics through the public API."""

    async def behaviour(
        requests: AsyncIterator[bytes], context: aio.ServicerContext[bytes, bytes]
    ) -> AsyncIterator[bytes]:
        await context.send_initial_metadata(())
        await anext(requests)  # Configure
        yield _bytes_field(_EVENT_OK, b'')
        await anext(requests)  # Feed
        if status is not None:
            assert message is not None
            trailers = () if cause is None else (('monty-cause', cause),)
            await context.abort(status, message, trailing_metadata=trailers)

    async with _serve_behaviour(behaviour) as url:
        async with AsyncMontyRemote(url, auto_resume=False) as pool:
            async with pool.checkout() as session:
                with pytest.raises(MontyDisconnectError) as caught:
                    await session.feed_run('1 + 1')
                err = caught.value
                assert err.stage == 'receive'
                assert err.status_code == ('OK' if status is None else status.name)
                assert err.status_message == message
                assert err.cause == cause
                assert err.transport_error is None
                assert str(err) == 'monty worker connection closed while waiting for a reply'
                with pytest.raises(AttributeError):
                    setattr(err, 'status_code', 'OK')


async def test_refused_stream_fails_the_checkout():
    """A server refusing the stream before admitting it — at capacity, say —
    fails the checkout with its status, never with a worker that then fails."""

    async def behaviour(
        _requests: AsyncIterator[bytes], context: aio.ServicerContext[bytes, bytes]
    ) -> AsyncIterator[bytes]:
        await context.abort(
            grpc.StatusCode.RESOURCE_EXHAUSTED, 'server at capacity', trailing_metadata=(('monty-cause', 'capacity'),)
        )
        yield b''  # pragma: no cover - makes this an async generator

    async with _serve_behaviour(behaviour) as url:
        async with AsyncMontyRemote(url) as pool:
            with pytest.raises(RuntimeError) as exc_info:
                async with pool.checkout():
                    pass
    assert exc_info.value.args[0] == snapshot(
        f'failed to spawn monty worker: {url}: RESOURCE_EXHAUSTED: server at capacity'
    )


async def test_out_of_memory_status_raises_memory_error():
    """A server reporting the worker ran out of memory ends the session with the
    same `MemoryError` a local out-of-memory kill raises."""

    async def behaviour(
        requests: AsyncIterator[bytes], context: aio.ServicerContext[bytes, bytes]
    ) -> AsyncIterator[bytes]:
        await context.send_initial_metadata(())
        await anext(requests)  # Configure
        yield _bytes_field(_EVENT_OK, b'')
        await anext(requests)  # Feed
        await context.abort(
            grpc.StatusCode.RESOURCE_EXHAUSTED,
            'worker exceeded its memory limit',
            trailing_metadata=(('monty-cause', 'out_of_memory'),),
        )

    async with _serve_behaviour(behaviour) as url:
        async with AsyncMontyRemote(url) as pool:
            async with pool.checkout() as session:
                with pytest.raises(MontyRuntimeError) as exc_info:
                    await session.feed_run("x = 'a' * 10**9")
    assert exc_info.value.display(format='msg') == snapshot('the worker exceeded its memory limit and was terminated')
