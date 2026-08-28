from __future__ import annotations

import time
from typing import Any

from inline_snapshot import snapshot

import pydantic_monty._monty as native
from pydantic_monty import Monty


class Adapter:
    def __init__(self) -> None:
        self.events: list[tuple[str, Any]] = []
        self.start_delay = 0.0

    def capture_context(self) -> tuple[str, str, int, str]:
        return ('00000000000000000000000000000001', '0000000000000002', 1, '')

    def start_span(
        self,
        trace_id: str,
        span_id: str,
        parent_id: str | None,
        trace_flags: int,
        trace_state: str,
        name: str,
        *args: Any,
    ) -> None:
        if self.start_delay:
            time.sleep(self.start_delay)
        self.events.append(('start', (trace_id, span_id, parent_id, trace_flags, trace_state, name)))

    def log(self, parent_id: str, level: str, message: str, *args: Any) -> None:
        self.events.append(('log', (parent_id, level, message)))

    def end_span(self, span_id: str, *args: Any) -> None:
        self.events.append(('end', span_id))

    def disable_root(self, trace_id: str, root_span_id: str) -> None:
        self.events.append(('disabled', (trace_id, root_span_id)))


def test_installed_telemetry_adapter_receives_session_tree():
    adapter = Adapter()
    native.__dict__['_install_telemetry_adapter'](1, adapter)

    with Monty() as pool:
        with pool.checkout() as session:
            assert session.feed_run('1 + 2') == snapshot(3)

    starts = [event[1] for event in adapter.events if event[0] == 'start']
    assert [event[0] for event in starts] == snapshot(
        ['00000000000000000000000000000001', '00000000000000000000000000000001']
    )
    assert starts[0][2] == snapshot('0000000000000002')
    assert starts[1][2] == starts[0][1]
    assert [(event[3], event[4]) for event in starts] == snapshot([(1, ''), (1, '')])
    assert [event[5] for event in starts] == snapshot(['session {script_name}', 'run code'])
    assert [event[0] for event in adapter.events] == snapshot(['start', 'start', 'end', 'end'])

    adapter.start_delay = 0.05
    started = time.monotonic()
    try:
        with Monty(request_timeout=0.005) as pool:
            with pool.checkout() as session:
                assert session.feed_run('4 + 5') == snapshot(9)
        assert time.monotonic() - started >= 0.05
    finally:
        adapter.start_delay = 0
