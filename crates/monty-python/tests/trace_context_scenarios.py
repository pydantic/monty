"""Scenarios for `test_trace_context.py`, each run in its own interpreter.

Telemetry is installed once per process, so a scenario with and one without it
cannot share one. Run as `python trace_context_scenarios.py <scenario>`; the
result is printed as JSON.
"""

from __future__ import annotations

import asyncio
import importlib.util
import json
import sys
from collections.abc import AsyncGenerator, AsyncIterator, Awaitable, Callable
from contextlib import aclosing, asynccontextmanager
from pathlib import Path
from typing import Any

from grpc import aio
from opentelemetry.sdk._logs import LoggerProvider
from opentelemetry.sdk.trace import ReadableSpan, TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter

from pydantic_monty import AsyncMontyRemote, instrument_telemetry
from pydantic_monty._binary import find_monty_binary

_RELAY_SCRIPT = Path(__file__).resolve().parents[3] / 'scripts' / 'grpc_relay.py'

# field numbers from `monty.proto`
_REQUEST_TRACEPARENT = 20
_TURN_STARTS = {1: 'configure', 3: 'feed', 8: 'load'}
_SUSPENSIONS = {2: 'call', 3: 'os call', 4: 'name lookup', 5: 'resolve futures'}
_EVENT_STREAMED_CALLS = 15
_EVENT_TRACEPARENT = 31
_STREAMED_CALLS_CALL = 1
_CALL_TRACEPARENT = 9


class TracingRelay:
    """A relay to a real `monty subprocess` child that stands in for a server recording spans.

    It keeps the `traceparent` of every turn-starting request, and names a span of
    its own on every suspension and streamed call, in the trace of the turn's request.
    """

    def __init__(self) -> None:
        self.sent: list[str | None] = []
        self.spans: dict[int, str] = {}
        self._trace_id = f'{0xFEED:032x}'

    @asynccontextmanager
    async def serve(self) -> AsyncGenerator[str]:
        spec = importlib.util.spec_from_file_location('grpc_relay', _RELAY_SCRIPT)
        assert spec is not None and spec.loader is not None
        relay = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(relay)
        behaviour = relay.relay_behaviour(find_monty_binary())

        async def handle(
            requests: AsyncIterator[bytes], context: aio.ServicerContext[bytes, bytes]
        ) -> AsyncIterator[bytes]:
            async with aclosing(behaviour(self._record(requests), context)) as events:
                async for event in events:
                    yield self._name_spans(event)

        server, url = await relay.start_server(relay.SessionHandler(handle), '127.0.0.1', 0)
        try:
            yield url
        finally:
            await server.stop(None)

    async def _record(self, requests: AsyncIterator[bytes]) -> AsyncIterator[bytes]:
        async for request in requests:
            fields = dict(_fields(request))
            if _TURN_STARTS.keys() & fields.keys():
                traceparent = fields.get(_REQUEST_TRACEPARENT)
                self.sent.append(None if traceparent is None else traceparent.decode())
                if traceparent is not None:
                    self._trace_id = traceparent.decode().split('-')[1]
            yield request

    def _name_spans(self, event: bytes) -> bytes:
        fields = _fields(event)
        kinds = [field for field, _ in fields if field < 20]
        if kinds == [_EVENT_STREAMED_CALLS]:
            calls = [
                _bytes_field(_STREAMED_CALLS_CALL, call + _bytes_field(_CALL_TRACEPARENT, self._span('streamed call')))
                for _, call in _fields(fields[0][1])
            ]
            return _bytes_field(_EVENT_STREAMED_CALLS, b''.join(calls)) + b''.join(
                _bytes_field(field, value) for field, value in fields[1:]
            )
        if kinds and kinds[0] in _SUSPENSIONS:
            return event + _bytes_field(_EVENT_TRACEPARENT, self._span(_SUSPENSIONS[kinds[0]]))
        return event

    def _span(self, kind: str) -> bytes:
        span_id = 0xC000 + len(self.spans)
        self.spans[span_id] = f'server {kind}'
        return f'00-{self._trace_id}-{span_id:016x}-01'.encode()


def _fields(buf: bytes) -> list[tuple[int, bytes]]:
    """The length-delimited fields of one protobuf message, in order; other fields are skipped."""
    fields: list[tuple[int, bytes]] = []
    i = 0
    while i < len(buf):
        key, i = _varint(buf, i)
        field, wire_type = key >> 3, key & 7
        if wire_type == 0:
            _, i = _varint(buf, i)
        elif wire_type == 2:
            length, i = _varint(buf, i)
            fields.append((field, buf[i : i + length]))
            i += length
        else:
            i += 8 if wire_type == 1 else 4
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


def _bytes_field(field: int, data: bytes) -> bytes:
    out = bytearray()
    for value in (field << 3 | 2, len(data)):
        while value > 0x7F:
            out.append(value & 0x7F | 0x80)
            value >>= 7
        out.append(value)
    return bytes(out) + data


class Spans:
    """The host's own tracer, and names for every span in the scenario."""

    def __init__(self, relay: TracingRelay) -> None:
        self.relay = relay
        self.exporter = InMemorySpanExporter()
        provider = TracerProvider()
        provider.add_span_processor(SimpleSpanProcessor(self.exporter))
        self.tracer = provider.get_tracer('trace-context-test')

    def finished(self) -> tuple[ReadableSpan, ...]:
        return self.exporter.get_finished_spans()

    def name(self, span_id: int | None) -> str | None:
        """The name of the span with `span_id`: the host's, or one the relay named."""
        if span_id is None:
            return None
        for span in self.finished():
            if span.context is not None and span.context.span_id == span_id:
                return span.name
        return self.relay.spans.get(span_id, f'unknown {span_id:x}')

    def parents(self) -> list[tuple[str, str | None]]:
        """Every finished host span with its parent's name, in the order they ended."""
        return [(span.name, self.name(span.parent.span_id if span.parent else None)) for span in self.finished()]

    def sent(self) -> list[str | None]:
        """The span each turn-starting request named."""
        return [None if value is None else self.name(int(value.split('-')[2], 16)) for value in self.relay.sent]

    def result(self) -> dict[str, Any]:
        return {'sent': self.sent(), 'spans': self.parents()}


async def _run(spans: Spans, feeds: Callable[[Any], Awaitable[None]]) -> dict[str, Any]:
    async with spans.relay.serve() as url:
        async with AsyncMontyRemote(url, request_timeout=30.0) as pool:
            with spans.tracer.start_as_current_span('checkout'):
                checkout = pool.checkout()
                await checkout.__aenter__()
            try:
                await feeds(checkout)
            finally:
                await checkout.__aexit__(None, None, None)
    return spans.result()


async def uninstrumented() -> dict[str, Any]:
    """No `instrument_telemetry()`: only `opentelemetry-api` reads and sets the current span."""
    return await _callbacks_under_server_spans(Spans(TracingRelay()))


async def logging_only() -> dict[str, Any]:
    """`instrument_telemetry()` with only a logger: Monty records spans no host tracer can resolve."""
    spans = Spans(TracingRelay())
    instrument_telemetry(logger=LoggerProvider().get_logger('trace-context-test'))
    return await _callbacks_under_server_spans(spans)


async def _callbacks_under_server_spans(spans: Spans) -> dict[str, Any]:
    """Feeds whose suspending, streamed and OS callbacks each open a span, under three callers."""
    tracer = spans.tracer

    def lookup(key: str) -> str:
        with tracer.start_as_current_span('in lookup'):
            return key.upper()

    async def fetch(key: str) -> str:
        with tracer.start_as_current_span('in fetch'):
            return key.upper()

    def os_handler(*, name: str, **_: Any) -> str:
        with tracer.start_as_current_span(f'in {name}'):
            return 'value'

    async def feeds(session: Any) -> None:
        with tracer.start_as_current_span('first caller'):
            assert await session.feed_run("lookup('a')", external_lookup={'lookup': lookup}) == 'A'
        with tracer.start_as_current_span('second caller'):
            code = "import asyncio\nawait asyncio.gather(fetch('b'), fetch('c'))"
            assert await session.feed_run(code, external_lookup={'fetch': fetch}) == ['B', 'C']
        with tracer.start_as_current_span('third caller'):
            assert await session.feed_run("import os\nos.getenv('KEY')", os=os_handler) == 'value'
        # no current span: the request names none
        assert await session.feed_run('1 + 1') == 2

    return await _run(spans, feeds)


async def instrumented() -> dict[str, Any]:
    """With `instrument_telemetry()`: the client records the session too."""
    spans = Spans(TracingRelay())
    tracer = spans.tracer
    instrument_telemetry(tracer=tracer)

    def lookup(key: str) -> str:
        with tracer.start_as_current_span('in lookup'):
            return key.upper()

    async def feeds(session: Any) -> None:
        for caller in ('first caller', 'second caller'):
            with tracer.start_as_current_span(caller):
                assert await session.feed_run("lookup('a')", external_lookup={'lookup': lookup}) == 'A'

    return await _run(spans, feeds)


async def without_opentelemetry() -> dict[str, Any]:
    """`opentelemetry-api` cannot be imported: nothing is sent, and callbacks still run."""
    relay = TracingRelay()
    for module in [name for name in sys.modules if name == 'opentelemetry' or name.startswith('opentelemetry.')]:
        sys.modules[module] = None  # pyright: ignore[reportArgumentType]

    def lookup(key: str) -> str:
        return key.upper()

    async with relay.serve() as url:
        async with AsyncMontyRemote(url, request_timeout=30.0) as pool:
            async with pool.checkout() as session:
                result = await session.feed_run("lookup('a')", external_lookup={'lookup': lookup})
    return {'sent': relay.sent, 'result': result, 'server_spans': sorted(set(relay.spans.values()))}


SCENARIOS = {
    'uninstrumented': uninstrumented,
    'logging_only': logging_only,
    'instrumented': instrumented,
    'without_opentelemetry': without_opentelemetry,
}

if __name__ == '__main__':
    print(json.dumps(asyncio.run(SCENARIOS[sys.argv[1]]())))
