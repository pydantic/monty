from __future__ import annotations

import asyncio
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from contextvars import ContextVar
from threading import Barrier
from typing import Any

import pytest
from inline_snapshot import snapshot
from opentelemetry import baggage, context, trace
from opentelemetry._logs import SeverityNumber
from opentelemetry.context import Context
from opentelemetry.sdk._logs import LoggerProvider
from opentelemetry.sdk._logs.export import InMemoryLogRecordExporter, SimpleLogRecordProcessor
from opentelemetry.sdk.metrics import MeterProvider
from opentelemetry.sdk.metrics.export import InMemoryMetricReader
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from opentelemetry.trace import NonRecordingSpan, SpanContext, StatusCode, TraceFlags, use_span

from pydantic_monty import (
    AsyncFunctionSnapshot,
    AsyncFutureSnapshot,
    AsyncMonty,
    AsyncNameLookupSnapshot,
    FunctionSnapshot,
    FutureSnapshot,
    Monty,
    MontyComplete,
    MontyRuntimeError,
    NameLookupSnapshot,
    instrument_telemetry,
)


class RecordingTracer:
    def __init__(self, tracer: Any) -> None:
        self.tracer = tracer
        self.start_delay = 0.0
        self.reject_next_start = False
        self.raise_on_start: int | None = None
        self.next_span_id = 100

    def start_span(self, name: str, **kwargs: Any) -> Any:
        if self.start_delay:
            time.sleep(self.start_delay)
        if self.raise_on_start is not None:
            self.raise_on_start -= 1
            if self.raise_on_start == 0:
                raise RuntimeError('telemetry failed')
        if self.reject_next_start:
            self.reject_next_start = False
            parent = trace.get_current_span(kwargs.get('context')).get_span_context()
            self.next_span_id += 1
            return NonRecordingSpan(
                SpanContext(
                    trace_id=parent.trace_id or 1,
                    span_id=self.next_span_id,
                    is_remote=False,
                    trace_flags=TraceFlags(0),
                )
            )
        return self.tracer.start_span(name, **kwargs)


_span_exporter = InMemorySpanExporter()
_tracer_provider = TracerProvider()
_tracer_provider.add_span_processor(SimpleSpanProcessor(_span_exporter))
_tracer = RecordingTracer(_tracer_provider.get_tracer('pydantic-monty-test'))
_metric_reader = InMemoryMetricReader()
_meter_provider = MeterProvider(metric_readers=[_metric_reader])
_log_exporter = InMemoryLogRecordExporter()
_logger_provider = LoggerProvider()
_logger_provider.add_log_record_processor(SimpleLogRecordProcessor(_log_exporter))
_installed = False


def install_telemetry() -> None:
    global _installed
    if not _installed:
        instrument_telemetry(
            tracer=_tracer,
            meter=_meter_provider.get_meter('pydantic-monty-test'),
            logger=_logger_provider.get_logger('pydantic-monty-test'),
        )
        _installed = True
    _span_exporter.clear()
    _log_exporter.clear()
    _tracer.start_delay = 0
    _tracer.reject_next_start = False
    _tracer.raise_on_start = None


def test_components_are_required():
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
import pydantic_monty._monty as native

try:
    native.__dict__['_install_telemetry'](None, None, None)
except ValueError as exc:
    assert str(exc) == 'at least one OpenTelemetry component is required'
else:
    raise AssertionError('expected telemetry installation to fail')
""",
        ],
        check=True,
    )


def test_metrics_can_be_disabled():
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
from opentelemetry import trace
from pydantic_monty import Monty, instrument_telemetry

instrument_telemetry(tracer=trace.get_tracer("test"))
with Monty() as pool:
    with pool.checkout() as session:
        assert session.feed_run("1 + 2") == 3
""",
        ],
        check=True,
    )


def test_standard_components_receive_session_tree():
    install_telemetry()
    parent_context = SpanContext(
        trace_id=1,
        span_id=2,
        is_remote=False,
        trace_flags=TraceFlags(1),
    )

    with use_span(NonRecordingSpan(parent_context)):
        with Monty() as pool:
            with pool.checkout(script_name='calculation.py') as session:
                assert session.feed_run("print('hello')\n1 + 2") == snapshot(3)

    spans = _span_exporter.get_finished_spans()
    assert [span.name for span in spans] == snapshot(['run code', 'session {script_name}'])
    run, session = spans
    assert session.parent is not None
    assert (session.parent.trace_id, session.parent.span_id) == snapshot((1, 2))
    assert run.parent is not None
    assert session.context is not None
    assert run.parent.span_id == session.context.span_id
    assert session.attributes is not None
    assert session.attributes['script_name'] == snapshot('calculation.py')
    assert run.attributes is not None
    assert run.attributes['code'] == snapshot("print('hello')\n1 + 2")
    assert run.attributes['sandbox.execution.code.attribute'] == snapshot('code')
    assert run.attributes['sandbox.execution.language'] == snapshot('python')
    assert run.attributes['output'] == snapshot(3)
    assert isinstance(run.start_time, int)
    assert isinstance(run.end_time, int)

    [log] = _log_exporter.get_finished_logs()
    assert log.log_record.body == snapshot('print stdout')
    assert log.log_record.severity_number == SeverityNumber.INFO
    assert run.context is not None
    assert log.log_record.trace_id == run.context.trace_id
    assert log.log_record.span_id == run.context.span_id
    # `code.*` is where in the host's source the record is emitted; it moves with every edit
    assert log.log_record.attributes is not None
    attributes = {k: v for k, v in log.log_record.attributes.items() if not k.startswith('code.')}
    assert attributes == snapshot(
        {
            'stream': 'stdout',
            'text': 'hello\n',
            'logfire.json_schema': '{"type":"object","properties":{"stream":{},"text":{},"length_limit_exceeded":{}}}',
            'thread.id': 1,
            'logfire.null_args': ('length_limit_exceeded',),
        }
    )

    with pytest.raises(RuntimeError, match='Monty telemetry is already configured'):
        instrument_telemetry(tracer=_tracer)


def test_standard_components_receive_errors():
    install_telemetry()

    with Monty() as pool:
        with pool.checkout() as session:
            with pytest.raises(MontyRuntimeError, match='division by zero'):
                session.feed_run('1 / 0')

    run, _session = _span_exporter.get_finished_spans()
    assert run.name == snapshot('run code')
    assert run.status.status_code is StatusCode.UNSET
    [error] = _log_exporter.get_finished_logs()
    assert error.log_record.body == snapshot('error ZeroDivisionError')
    assert error.log_record.severity_number is SeverityNumber.ERROR
    assert error.log_record.attributes is not None
    assert error.log_record.attributes['exc_type'] == snapshot('ZeroDivisionError')
    assert error.log_record.attributes['exc_message'] == snapshot('division by zero')
    assert error.log_record.attributes['traceback'] == snapshot('<python-input-0>:1 in <module>')


def test_concurrent_checkouts_do_not_deadlock_components():
    install_telemetry()
    _tracer.start_delay = 0.01
    barrier = Barrier(2)

    with Monty(min_processes=2, max_processes=2) as pool:

        def run(value: int) -> int:
            barrier.wait()
            with pool.checkout() as session:
                return session.feed_run('value + 1', inputs={'value': value})

        with ThreadPoolExecutor(max_workers=2) as executor:
            assert sorted(executor.map(run, range(2))) == snapshot([1, 2])


def test_tracer_can_reenter_monty():
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
from opentelemetry.sdk.trace import TracerProvider
from pydantic_monty import Monty, instrument_telemetry

class ReentrantTracer:
    def __init__(self):
        self.tracer = TracerProvider().get_tracer('test')
        self.pool = None
        self.reentered = False

    def start_span(self, *args, **kwargs):
        if self.pool is not None and not self.reentered:
            self.reentered = True
            with self.pool.checkout() as session:
                assert session.feed_run('20 + 22') == 42
        return self.tracer.start_span(*args, **kwargs)

tracer = ReentrantTracer()
instrument_telemetry(tracer=tracer)
with Monty() as nested_pool:
    tracer.pool = nested_pool
    with Monty() as pool:
        with pool.checkout() as session:
            assert session.feed_run('1 + 2') == 3
""",
        ],
        check=True,
        timeout=30,
    )


def test_tracer_can_reject_one_root():
    install_telemetry()
    _tracer.reject_next_start = True

    with Monty() as pool:
        with pool.checkout() as session:
            assert session.feed_run('1 + 2') == snapshot(3)
        with pool.checkout() as session:
            assert session.feed_run('4 + 5') == snapshot(9)

    assert [span.name for span in _span_exporter.get_finished_spans()] == snapshot(
        ['run code', 'session {script_name}']
    )


def test_standard_meter_receives_metrics():
    install_telemetry()

    with Monty(min_processes=1, max_processes=1) as pool:
        with pool.checkout() as session:
            assert session.feed_run("print('hi')\n6 * 7") == snapshot(42)

    metrics = _metric_reader.get_metrics_data()
    assert metrics is not None
    instruments = [
        metric for resource in metrics.resource_metrics for scope in resource.scope_metrics for metric in scope.metrics
    ]
    assert sorted({metric.name for metric in instruments}) == snapshot(
        [
            'monty.pool.checkout.wait',
            'monty.pool.session.duration',
            'monty.pool.worker.terminated',
            'monty.pool.workers.idle',
            'monty.pool.workers.live',
            'monty.print.bytes',
            'monty.run.duration',
            'monty.run.execution_time',
            'monty.turn.duration',
            'monty.wire.frame.bytes',
        ]
    )
    [run] = [metric for metric in instruments if metric.name == 'monty.run.duration']
    assert (run.unit, run.description) == snapshot(
        ('s', 'Wall time of one feed, including time spent waiting on the host.')
    )
    run_point = next(point for point in run.data.data_points if point.attributes == {'outcome': 'complete'})
    assert run_point.attributes == snapshot({'outcome': 'complete'})
    assert getattr(run_point, 'sum') > 0


@pytest.mark.parametrize('fail', [False, True])
async def test_eager_coroutine_result_is_recorded_on_the_call_span(fail: bool):
    """An eager value or exception closes the call span without a future-results record."""
    install_telemetry()

    async def fetch() -> int:
        await asyncio.sleep(0)
        if fail:
            raise ValueError('failed')
        return 42

    # the `é` makes byte offsets differ from character offsets
    code = '# é\ntry:\n    result = await fetch()\nexcept ValueError:\n    result = 0\nresult'
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            assert await session.feed_run(code, external_lookup={'fetch': fetch}) == (0 if fail else 42)

    spans = _span_exporter.get_finished_spans()
    assert [span.name for span in spans] == ['call {function_name}', 'run code', 'session {script_name}']
    call, run, _session = spans
    assert call.attributes is not None
    assert call.attributes['function_name'] == 'fetch'
    assert call.attributes['return_value'] == ('raise ValueError: failed' if fail else 42)
    assert {k: v for k, v in call.attributes.items() if k.startswith('sandbox.')} == snapshot(
        {
            'sandbox.code.file.path': '<python-input-0>',
            'sandbox.code.offset.start': 29,
            'sandbox.code.offset.end': 36,
        }
    )
    assert call.parent is not None and run.context is not None
    assert call.parent.span_id == run.context.span_id
    assert _log_exporter.get_finished_logs() == ()


async def test_deferred_coroutine_keeps_future_resolution_telemetry():
    """A stored awaitable still has a separate wait span and future-results record."""
    install_telemetry()

    async def fetch() -> int:
        return 42

    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('pending = fetch()\nawait pending', external_lookup={'fetch': fetch}) == 42

    spans = _span_exporter.get_finished_spans()
    assert [span.name for span in spans] == [
        'call {function_name}',
        'resolve futures',
        'run code',
        'session {script_name}',
    ]
    call, waiting, _run, _session = spans
    assert call.attributes is not None
    assert call.attributes['return_value'] == f'future {call.attributes["call_id"]}'
    [log] = _log_exporter.get_finished_logs()
    assert log.log_record.body == 'future results'
    assert waiting.context is not None
    assert log.log_record.span_id == waiting.context.span_id


@pytest.mark.parametrize('kind', ['function', 'os', 'name', 'future'])
def test_snapshot_trace_context(kind: str):
    install_telemetry()
    tracer = _tracer_provider.get_tracer('manual-snapshot')
    code = {
        'function': 'callback()',
        'os': "from pathlib import Path\nPath('/file').exists()",
        'name': 'missing',
        'future': 'await callback()',
    }[kind]
    marker = ContextVar('marker', default='caller')

    with tracer.start_as_current_span('host') as host, Monty() as pool:
        with pool.checkout() as session:
            feed_context = context.set_value('custom_entry', 'feed value', baggage.set_baggage('request', 'feed'))
            token = context.attach(feed_context)
            try:
                paused = session.feed_start(code)
            finally:
                context.detach(token)
            if kind == 'future':
                assert isinstance(paused, FunctionSnapshot)
                paused = paused.resume({'future': ...})
            assert not isinstance(paused, MontyComplete)
            saved = paused.trace_context()
            assert isinstance(saved, Context)
            assert (trace.get_current_span() is host) == snapshot(True)
            assert baggage.get_baggage('request', saved) == snapshot('feed')
            assert context.get_value('custom_entry', saved) == snapshot('feed value')
            assert baggage.get_baggage('request') == snapshot(None)
            suspension = trace.get_current_span(saved)
            assert (suspension is host) == snapshot(False)
            token = context.attach(saved)
            try:
                assert (trace.get_current_span() is suspension) == snapshot(True)
                assert marker.get() == snapshot('caller')
                marker.set('handler')
                with tracer.start_as_current_span('handler'):
                    pass
            finally:
                context.detach(token)
            assert (trace.get_current_span() is host) == snapshot(True)
            assert marker.get() == snapshot('handler')
            assert suspension.is_recording() == snapshot(True)
            with pytest.raises(ValueError) as exc_info:
                token = context.attach(saved)
                try:
                    raise ValueError('handler failed')
                finally:
                    context.detach(token)
            assert str(exc_info.value) == snapshot('handler failed')
            assert (trace.get_current_span() is host) == snapshot(True)
            assert suspension.is_recording() == snapshot(True)
            if isinstance(paused, FunctionSnapshot):
                result = paused.resume({'return_value': 42})
            elif isinstance(paused, NameLookupSnapshot):
                result = paused.resume(value=42)
            else:
                assert isinstance(paused, FutureSnapshot)
                result = paused.resume({paused.pending_call_ids[0]: {'return_value': 42}})
            assert isinstance(result, MontyComplete)
            assert result.output == snapshot(42)
            assert suspension.is_recording() == snapshot(False)
            with pytest.raises(RuntimeError) as exc_info:
                paused.trace_context()
            assert str(exc_info.value) == snapshot('snapshot has already been resumed')
            token = context.attach(saved)
            try:
                assert (trace.get_current_span() is suspension) == snapshot(True)
            finally:
                context.detach(token)
            assert (trace.get_current_span() is host) == snapshot(True)

    spans = _span_exporter.get_finished_spans()
    handler = next(span for span in spans if span.name == 'handler')
    assert handler.parent is not None
    assert (handler.parent == suspension.get_span_context()) == snapshot(True)
    parent = next(span for span in spans if span.context == handler.parent)
    assert (
        parent.name
        == {
            'function': 'call {function_name}',
            'os': 'os call {function}',
            'name': 'name lookup {name}',
            'future': 'resolve futures',
        }[kind]
    )


@pytest.mark.parametrize('kind', ['function', 'name', 'future'])
async def test_async_snapshot_trace_context(kind: str):
    install_telemetry()
    tracer = _tracer_provider.get_tracer('manual-snapshot')
    code = {'function': 'callback()', 'name': 'missing', 'future': 'await callback()'}[kind]
    ready = asyncio.Event()
    entered_count = 0

    async def handle(index: int):
        nonlocal entered_count
        with tracer.start_as_current_span(f'host {index}') as host:
            async with pool.checkout() as session:
                token = context.attach(baggage.set_baggage('request', str(index)))
                try:
                    started = session.feed_start(code)
                finally:
                    context.detach(token)
                paused = await started
                if kind == 'future':
                    assert isinstance(paused, AsyncFunctionSnapshot)
                    paused = await paused.resume({'future': ...})
                assert not isinstance(paused, MontyComplete)
                saved = paused.trace_context()
                assert (baggage.get_baggage('request', saved) == str(index)) == snapshot(True)
                assert baggage.get_baggage('request') == snapshot(None)
                token = context.attach(saved)
                try:
                    suspension = trace.get_current_span()
                    entered_count += 1
                    if entered_count == 2:
                        ready.set()
                    await ready.wait()
                    await asyncio.sleep(0)
                    assert (trace.get_current_span() is suspension) == snapshot(True)
                    assert (baggage.get_baggage('request') == str(index)) == snapshot(True)
                    with tracer.start_as_current_span(f'handler {index}'):
                        await asyncio.sleep(0)
                finally:
                    context.detach(token)
                assert (trace.get_current_span() is host) == snapshot(True)
                assert suspension.is_recording() == snapshot(True)
                if isinstance(paused, AsyncFunctionSnapshot):
                    result = await paused.resume({'return_value': index})
                elif isinstance(paused, AsyncNameLookupSnapshot):
                    result = await paused.resume(value=index)
                else:
                    assert isinstance(paused, AsyncFutureSnapshot)
                    result = await paused.resume({paused.pending_call_ids[0]: {'return_value': index}})
                assert isinstance(result, MontyComplete)
                assert (result.output == index) == snapshot(True)
                with pytest.raises(RuntimeError) as exc_info:
                    paused.trace_context()
                assert str(exc_info.value) == snapshot('snapshot has already been resumed')
                assert (trace.get_current_span() is host) == snapshot(True)
                return suspension.get_span_context()

    async with AsyncMonty(min_processes=2, max_processes=2) as pool:
        parents = await asyncio.gather(handle(1), handle(2))
    spans = _span_exporter.get_finished_spans()
    for index, parent in enumerate(parents, 1):
        handler = next(span for span in spans if span.name == f'handler {index}')
        assert (handler.parent == parent) == snapshot(True)
    assert (parents[0] != parents[1]) == snapshot(True)


def test_restored_snapshot_trace_context():
    install_telemetry()
    tracer = _tracer_provider.get_tracer('manual-snapshot')
    with Monty() as pool:
        with pool.checkout() as session:
            token = context.attach(baggage.set_baggage('request', 'original'))
            try:
                paused = session.feed_start('callback()')
            finally:
                context.detach(token)
            assert isinstance(paused, FunctionSnapshot)
            original = trace.get_current_span(paused.trace_context()).get_span_context()
            state = paused.dump()
        with pool.checkout() as session:
            token = context.attach(baggage.set_baggage('request', 'restored'))
            try:
                paused = session.load_snapshot(state)
            finally:
                context.detach(token)
            assert isinstance(paused, FunctionSnapshot)
            saved = paused.trace_context()
            assert baggage.get_baggage('request', saved) == snapshot('restored')
            restored = trace.get_current_span(saved).get_span_context()
            token = context.attach(saved)
            try:
                with tracer.start_as_current_span('restored handler'):
                    pass
            finally:
                context.detach(token)
            result = paused.resume({'return_value': 42})
            assert isinstance(result, MontyComplete)
            assert result.output == snapshot(42)
    handler = next(span for span in _span_exporter.get_finished_spans() if span.name == 'restored handler')
    assert (handler.parent == restored) == snapshot(True)
    assert (restored != original) == snapshot(True)


async def test_async_restored_snapshot_trace_context():
    install_telemetry()
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            token = context.attach(baggage.set_baggage('request', 'original'))
            try:
                started = session.feed_start('callback()')
            finally:
                context.detach(token)
            paused = await started
            assert isinstance(paused, AsyncFunctionSnapshot)
            state = paused.dump()
        async with pool.checkout() as session:
            token = context.attach(baggage.set_baggage('request', 'restored'))
            try:
                loading = session.load_snapshot(state)
            finally:
                context.detach(token)
            paused = await loading
            assert isinstance(paused, AsyncFunctionSnapshot)
            saved = paused.trace_context()
            assert baggage.get_baggage('request', saved) == snapshot('restored')
            assert baggage.get_baggage('request') == snapshot(None)
            result = await paused.resume({'return_value': 42})
            assert isinstance(result, MontyComplete)
            assert result.output == snapshot(42)


@pytest.mark.parametrize('mode', ['disabled', 'missing', 'sampled-out', 'broken-context'])
def test_snapshot_trace_context_telemetry_fallback(mode: str):
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
import sys

mode = sys.argv[1]
if mode == 'missing':
    sys.modules['opentelemetry'] = None

from pydantic_monty import FunctionSnapshot, Monty, instrument_telemetry

if mode != 'missing':
    from opentelemetry import context, trace
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.sdk.trace.sampling import ALWAYS_OFF
    provider = TracerProvider(sampler=ALWAYS_OFF)
    tracer = provider.get_tracer('manual-snapshot')
    host = trace.NonRecordingSpan(trace.SpanContext(1, 2, False, trace.TraceFlags(1)))
    token = context.attach(trace.set_span_in_context(host))
    if mode != 'disabled':
        instrument_telemetry(tracer=tracer)

with Monty() as pool:
    with pool.checkout() as session:
        paused = session.feed_start('callback()')
        assert isinstance(paused, FunctionSnapshot)
        if mode == 'missing':
            try:
                paused.trace_context()
            except ImportError as exc:
                assert str(exc) == 'trace_context() requires opentelemetry-api; install it with pip install opentelemetry-api'
            else:
                raise AssertionError('expected ImportError')
        else:
            captured = context.get_current()
            caller_token = context.attach(context.set_value('after feed', 'caller'))
            original_set_span = trace.set_span_in_context
            if mode == 'broken-context':
                def broken_set_span(*args):
                    raise RuntimeError('context failed')
                trace.set_span_in_context = broken_set_span
            saved = paused.trace_context()
            trace.set_span_in_context = original_set_span
            assert context.get_value('after feed', saved) is None
            assert isinstance(saved, context.Context)
            assert trace.get_current_span() is host
            span = trace.get_current_span(saved)
            if mode == 'sampled-out':
                assert span is not host
                assert not span.is_recording()
            else:
                assert saved is captured
            context.detach(caller_token)
        assert paused.resume({'return_value': 42}).output == 42
if mode != 'missing':
    context.detach(token)
""",
            mode,
        ],
        check=True,
        timeout=30,
    )


def test_logger_failure_does_not_disable_spans():
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
import sys

from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from pydantic_monty import Monty, instrument_telemetry

class BrokenLogger:
    def emit(self, *args, **kwargs):
        raise RuntimeError("logging failed")

exporter = InMemorySpanExporter()
provider = TracerProvider()
provider.add_span_processor(SimpleSpanProcessor(exporter))
instrument_telemetry(tracer=provider.get_tracer("test"), logger=BrokenLogger())
sys.unraisablehook = lambda args: None
with Monty() as pool:
    with pool.checkout() as session:
        assert session.feed_run("print('hello')\\n1 + 2") == 3
assert [span.name for span in exporter.get_finished_spans()] == ["run code", "session {script_name}"]
""",
        ],
        check=True,
    )


def test_tracer_failure_does_not_disable_logging():
    subprocess.run(
        [
            sys.executable,
            '-c',
            """
import sys

from opentelemetry.sdk._logs import LoggerProvider
from opentelemetry.sdk._logs.export import InMemoryLogRecordExporter, SimpleLogRecordProcessor
from opentelemetry.sdk.trace import TracerProvider
from pydantic_monty import Monty, instrument_telemetry

class BrokenTracer:
    def __init__(self):
        self.tracer = TracerProvider().get_tracer('test')
        self.starts = 0

    def start_span(self, *args, **kwargs):
        self.starts += 1
        if self.starts == 2:
            raise RuntimeError('telemetry failed')
        return self.tracer.start_span(*args, **kwargs)

exporter = InMemoryLogRecordExporter()
provider = LoggerProvider()
provider.add_log_record_processor(SimpleLogRecordProcessor(exporter))
unraisable = []
sys.unraisablehook = unraisable.append
instrument_telemetry(tracer=BrokenTracer(), logger=provider.get_logger('test'))
with Monty() as pool:
    with pool.checkout() as session:
        assert session.feed_run("print('still logged')\\n1 + 2") == 3
assert str(unraisable[0].exc_value) == 'telemetry failed'
[log] = exporter.get_finished_logs()
assert log.log_record.body == 'print stdout'
assert log.log_record.attributes['text'] == 'still logged\\n'
""",
        ],
        check=True,
    )
