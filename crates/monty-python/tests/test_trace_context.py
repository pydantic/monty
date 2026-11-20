"""Trace context per turn: each feed names its caller's span to the server, and a
host callback runs under the span the server named for its suspension.

Each scenario runs in its own interpreter (see `trace_context_scenarios.py`).
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest
from inline_snapshot import snapshot

# as `test_remote.py`: the relay needs grpcio, which free-threaded CI jobs lack
pytestmark = pytest.mark.skipif(importlib.util.find_spec('grpc') is None, reason='needs grpcio')

_SCENARIOS = Path(__file__).with_name('trace_context_scenarios.py')


def run_scenario(name: str) -> dict[str, Any]:
    # stderr is inherited, so a failing scenario's traceback reaches pytest's output
    result = subprocess.run([sys.executable, str(_SCENARIOS), name], check=True, stdout=subprocess.PIPE, text=True)
    return json.loads(result.stdout)


def test_feeds_and_callbacks_join_the_callers_and_servers_spans():
    """Without `instrument_telemetry()`, each feed names the span it was called under,
    and a span opened in a host callback is a child of the server's span for the call."""
    assert run_scenario('uninstrumented') == snapshot(
        {
            'sent': ['first caller', 'second caller', 'third caller', None],
            'spans': [
                ['checkout', None],
                ['in lookup', 'server call'],
                ['first caller', None],
                ['in fetch', 'server streamed call'],
                ['in fetch', 'server streamed call'],
                ['second caller', None],
                ['in os.getenv', 'server os call'],
                ['third caller', None],
            ],
        }
    )


def test_logging_only_instrumentation_keeps_callbacks_under_server_spans():
    """With `instrument_telemetry(logger=...)` alone, Monty's recorded spans have no host
    span, so a host callback still runs under the server's span for its call."""
    assert run_scenario('logging_only') == snapshot(
        {
            'sent': ['first caller', 'second caller', 'third caller', None],
            'spans': [
                ['checkout', None],
                ['in lookup', 'server call'],
                ['first caller', None],
                ['in fetch', 'server streamed call'],
                ['in fetch', 'server streamed call'],
                ['second caller', None],
                ['in os.getenv', 'server os call'],
                ['third caller', None],
            ],
        }
    )


def test_instrumented_feeds_nest_under_their_callers():
    """With `instrument_telemetry()`, each `run code` span is a child of its own caller
    and is the span the request names; the session span is the parent of none."""
    assert run_scenario('instrumented') == snapshot(
        {
            'sent': ['run code', 'run code'],
            'spans': [
                ['checkout', None],
                ['in lookup', 'call {function_name}'],
                ['call {function_name}', 'server call'],
                ['run code', 'first caller'],
                ['first caller', None],
                ['in lookup', 'call {function_name}'],
                ['call {function_name}', 'server call'],
                ['run code', 'second caller'],
                ['second caller', None],
                ['session {script_name}', 'checkout'],
            ],
        }
    )


def test_nothing_is_sent_without_opentelemetry():
    """Without `opentelemetry-api`, requests name no span and callbacks run as usual,
    even though the server names a span on the suspension."""
    assert run_scenario('without_opentelemetry') == snapshot(
        {'sent': [None], 'result': 'A', 'server_spans': ['server call']}
    )
