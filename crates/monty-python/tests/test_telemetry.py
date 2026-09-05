from __future__ import annotations

import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from threading import Barrier
from typing import Any

import pytest
from inline_snapshot import snapshot
from opentelemetry import trace
from opentelemetry._logs import SeverityNumber
from opentelemetry.sdk._logs import LoggerProvider
from opentelemetry.sdk._logs.export import InMemoryLogRecordExporter, SimpleLogRecordProcessor
from opentelemetry.sdk.metrics import MeterProvider
from opentelemetry.sdk.metrics.export import InMemoryMetricReader
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.sdk.trace.export.in_memory_span_exporter import InMemorySpanExporter
from opentelemetry.trace import NonRecordingSpan, SpanContext, StatusCode, TraceFlags, use_span

from pydantic_monty import Monty, MontyRuntimeError, instrument_telemetry


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
    assert run.attributes['output'] == snapshot(3)
    assert isinstance(run.start_time, int)
    assert isinstance(run.end_time, int)

    [log] = _log_exporter.get_finished_logs()
    assert log.log_record.body == snapshot('print stdout')
    assert log.log_record.severity_number == SeverityNumber.INFO
    assert run.context is not None
    assert log.log_record.trace_id == run.context.trace_id
    assert log.log_record.span_id == run.context.span_id
    assert log.log_record.attributes == snapshot(
        {
            'stream': 'stdout',
            'text': 'hello\n',
            'logfire.json_schema': '{"type":"object","properties":{"stream":{},"text":{},"length_limit_exceeded":{}}}',
            'thread.id': 1,
            'code.file.path': 'crates/monty-pool/src/telemetry/tracing.rs',
            'code.line.number': 272,
            'code.module.name': 'monty_pool::telemetry::tracing',
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
