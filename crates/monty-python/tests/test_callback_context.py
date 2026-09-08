from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest


@pytest.mark.parametrize('mode', ['sampled-out', 'broken-attach', 'broken-tracer'])
def test_callback_context_telemetry_fallback(mode: str):
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
import asyncio
import sys
from contextvars import ContextVar
from opentelemetry import context, trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.sampling import ALWAYS_OFF
from pydantic_monty import AsyncMonty, instrument_telemetry

mode = sys.argv[1]
provider = TracerProvider(sampler=ALWAYS_OFF)
tracer = provider.get_tracer('callbacks')
host_tracer = TracerProvider().get_tracer('host')
spans = []
class RecordingTracer:
    def start_span(self, *args, **kwargs):
        if mode == 'broken-tracer':
            raise RuntimeError('tracer failed')
        span = tracer.start_span(*args, **kwargs)
        spans.append(span)
        return span
instrument_telemetry(tracer=RecordingTracer())
request = ContextVar('request', default='outside')

async def main():
    request.set('caller')
    with host_tracer.start_as_current_span('host') as host:
        original_attach = context.attach
        if mode == 'broken-attach':
            def broken_attach(*args):
                raise RuntimeError('attach failed')
            context.attach = broken_attach
        seen = []
        def printed(stream, text):
            assert request.get() == 'caller'
            current = trace.get_current_span()
            assert current is (spans[-1] if mode == 'sampled-out' else host)
            if mode == 'sampled-out':
                assert not current.is_recording()
            seen.append(text)
        try:
            async with AsyncMonty() as pool:
                async with pool.checkout() as session:
                    await session.feed_run("print('hello')", print_callback=printed)
            assert seen == ['hello\\n']
            assert trace.get_current_span() is host
        finally:
            context.attach = original_attach
asyncio.run(main())
assert request.get() == 'outside'
""",
            mode,
        ],
        check=True,
        timeout=60,
    )


@pytest.mark.parametrize('transport', ['subprocess', 'websocket'])
def test_callback_contexts(transport: str):
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
import asyncio
from contextvars import ContextVar

from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from pydantic_monty import AsyncMonty, AsyncMontyWebsocket, Monty, instrument_telemetry
from opentelemetry import baggage, context

exporter = InMemorySpanExporter()
provider = TracerProvider()
provider.add_span_processor(SimpleSpanProcessor(exporter))
tracer = provider.get_tracer('callbacks')
instrument_telemetry(tracer=tracer)
request = ContextVar('request', default='outside')

async def run(index, url=None):
    request.set(index)
    token = context.attach(baggage.set_baggage('request', str(index)))
    with tracer.start_as_current_span(f'host {index}'):
        host = trace.get_current_span()
        def printed(stream, text):
            assert request.get() == index
            assert baggage.get_baggage('request') == str(index)
            with tracer.start_as_current_span(f'print {index}'):
                pass
        def sync_callback():
            assert request.get() == index
            assert baggage.get_baggage('request') == str(index)
            with tracer.start_as_current_span(f'sync {index}'):
                pass
            return 10
        async def async_callback():
            assert request.get() == index
            assert baggage.get_baggage('request') == str(index)
            with tracer.start_as_current_span(f'async {index}'):
                await asyncio.sleep(0.01)
                assert request.get() == index
            return 20
        def os_callback(function, args, kwargs):
            assert request.get() == index
            assert baggage.get_baggage('request') == str(index)
            with tracer.start_as_current_span(f'os {index}'):
                pass
            return True
        async with (AsyncMontyWebsocket(url) if url else AsyncMonty()) as pool:
            async with pool.checkout(script_name=str(index)) as session:
                assert await session.feed_run(
                    "print('hello'); sync_callback() + await async_callback()",
                    external_lookup={'sync_callback': sync_callback, 'async_callback': async_callback},
                    print_callback=printed,
                ) == 30
                assert await session.feed_run("from pathlib import Path; Path('/x').exists()", os=os_callback)
        assert trace.get_current_span() is host
    context.detach(token)

async def main():
    import sys
    if sys.argv[1] == 'subprocess':
        await asyncio.gather(run(1), run(2))
    else:
        import runpy
        from websockets.asyncio.server import serve
        from pydantic_monty._binary import find_monty_binary
        bridge = runpy.run_path(sys.argv[2])['bridge_connection']
        async def handler(websocket):
            await bridge(websocket, find_monty_binary())
        async with serve(handler, '127.0.0.1', 0, max_size=None) as server:
            port = server.sockets[0].getsockname()[1]
            url = f'ws://127.0.0.1:{port}'
            await asyncio.gather(run(1, url), run(2, url))

asyncio.run(main())
assert request.get() == 'outside'
spans = exporter.get_finished_spans()
by_id = {s.context.span_id: s for s in spans}
for index in (1, 2):
    for kind, expected in [('print', 'run code'), ('sync', 'call {function_name}'), ('async', 'call {function_name}'), ('os', 'os call {function}')]:
        span = next(s for s in spans if s.name == f'{kind} {index}')
        parent = by_id[span.parent.span_id]
        assert parent.name == expected, (span.name, parent.name)
        while parent.parent is not None:
            parent = by_id[parent.parent.span_id]
        assert parent.name == f'host {index}'

with tracer.start_as_current_span('sync host') as host:
    request.set('sync')
    def callback(*args):
        assert request.get() == 'sync'
        with tracer.start_as_current_span('sync child'):
            pass
        return 42
    with Monty() as pool:
        with pool.checkout() as session:
            assert session.feed_run('callback()', external_lookup={'callback': callback}) == 42
    assert trace.get_current_span() is host
spans = exporter.get_finished_spans()
child = next(s for s in spans if s.name == 'sync child')
assert next(s for s in spans if s.context.span_id == child.parent.span_id).name == 'call {function_name}'
""",
            transport,
            str(Path(__file__).resolve().parents[3] / 'scripts' / 'websocket_relay.py'),
        ],
        check=True,
        timeout=60,
    )


def test_uninstrumented_callback_context():
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
import asyncio
from contextvars import ContextVar
from pydantic_monty import AsyncMonty, MontyRuntimeError

request = ContextVar('request', default='outside')

async def main():
    request.set('caller')
    seen = []
    def printed(stream, text):
        seen.append(request.get())
        request.set('callback only')
    def fail():
        assert request.get() == 'caller'
        request.set('callback only')
        raise ValueError('callback failed')
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            await session.feed_run("print('first')", print_callback=printed)
            assert request.get() == 'caller'
            try:
                await session.feed_run('fail()', external_lookup={'fail': fail})
            except MontyRuntimeError:
                pass
            else:
                raise AssertionError('expected callback failure')
            assert request.get() == 'caller'
            await session.feed_run("print('second')", print_callback=printed)
    assert seen == ['caller', 'caller']

asyncio.run(main())
assert request.get() == 'outside'
""",
        ],
        check=True,
        timeout=60,
    )


def test_snapshot_callback_context():
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
import asyncio
from contextvars import ContextVar
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from pydantic_monty import AsyncMonty, Monty, MontyComplete, instrument_telemetry

exporter = InMemorySpanExporter()
provider = TracerProvider()
provider.add_span_processor(SimpleSpanProcessor(exporter))
tracer = provider.get_tracer('callbacks')
instrument_telemetry(tracer=tracer)
request = ContextVar('request', default='outside')

async def main():
    async def callback():
        assert request.get() == 'resume'
        with tracer.start_as_current_span('async callback'):
            await asyncio.sleep(0)
            assert request.get() == 'resume'
        return 42
    def printed(stream, text):
        assert request.get() == 'resume'
        with tracer.start_as_current_span('snapshot print'):
            pass
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            request.set('feed')
            snapshot = await session.feed_start(
                "value = await callback(); print(value); value",
                external_lookup={'callback': callback}, print_callback=printed,
            )
            request.set('resume')
            while not isinstance(snapshot, MontyComplete):
                snapshot = await snapshot.resume_auto()
            assert snapshot.output == 42

asyncio.run(main())
request.set('sync')
def callback():
    assert request.get() == 'sync'
    with tracer.start_as_current_span('sync callback'):
        pass
    return 42
with Monty() as pool:
    with pool.checkout() as session:
        snapshot = session.feed_start('callback()', external_lookup={'callback': callback})
        assert snapshot.resume_auto().output == 42

spans = exporter.get_finished_spans()
by_id = {span.context.span_id: span for span in spans}
for name, parent in [('async callback', 'call {function_name}'), ('sync callback', 'call {function_name}'), ('snapshot print', 'run code')]:
    span = next(span for span in spans if span.name == name)
    assert by_id[span.parent.span_id].name == parent
""",
        ],
        check=True,
        timeout=60,
    )


@pytest.mark.parametrize('kind', ['external', 'os', 'print', 'method', 'attribute'])
@pytest.mark.parametrize('failure', ['none', 'enter', 'record', 'disabled'])
def test_synchronous_callback_exception_recording(kind: str, failure: str):
    subprocess.run(
        [
            sys.executable,
            '-c',
            r"""
import sys
from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from pydantic_monty import ClassInstance, Monty, MontyRuntimeError, instrument_telemetry

kind, failure = sys.argv[1:]
exporter = InMemorySpanExporter()
provider = TracerProvider()
provider.add_span_processor(SimpleSpanProcessor(exporter))
tracer = provider.get_tracer('callbacks')
if failure != 'disabled':
    instrument_telemetry(tracer=tracer)
calls = []

def fail(*args):
    span = trace.get_current_span()
    assert span.is_recording()
    calls.append(span.get_span_context().span_id)
    if failure == 'record':
        def broken_record(*args, **kwargs):
            raise RuntimeError('telemetry failed')
        span.record_exception = broken_record
    raise ValueError('callback failed')

class HostObject:
    method = fail
    attribute = property(fail)

with tracer.start_as_current_span('host') as host:
    if failure == 'enter':
        def broken_use_span(*args, **kwargs):
            raise RuntimeError('telemetry failed')
        trace.use_span = broken_use_span
    with Monty() as pool:
        with pool.checkout() as session:
            if kind == 'print':
                try:
                    session.feed_run("print('hello')", print_callback=fail)
                except MontyRuntimeError as exc:
                    assert str(exc) == 'ValueError: callback failed'
                else:
                    raise AssertionError('expected print failure')
            else:
                expression = {
                    'external': 'fail()',
                    'os': "Path('/foo').exists()",
                    'method': 'obj.method()',
                    'attribute': 'obj.attribute',
                }[kind]
                code = "from pathlib import Path\ntry:\n    " + expression + "\nexcept ValueError:\n    result = 42\nresult"
                obj = ClassInstance(HostObject(), allowed_methods='all', lazy_attrs='all')
                assert session.feed_run(code, inputs={'obj': obj}, external_lookup={'fail': fail}, os=fail) == 42
    assert trace.get_current_span() is host
    assert host.is_recording()
assert len(calls) == 1
spans = exporter.get_finished_spans()
span, = [s for s in spans if s.context.span_id == calls[0]]
if failure != 'disabled':
    assert span.context.span_id != host.get_span_context().span_id
assert not host.events
if failure == 'none':
    event, = span.events
    assert event.name == 'exception'
    assert event.attributes['exception.type'] == 'ValueError'
    assert event.attributes['exception.message'] == 'callback failed'
    assert 'in fail' in event.attributes['exception.stacktrace']
    assert span.status.status_code == trace.StatusCode.ERROR
else:
    assert not span.events
""",
            kind,
            failure,
        ],
        check=True,
    )
