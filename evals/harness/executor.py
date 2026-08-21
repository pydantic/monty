"""Runs agent-written code inside Monty and records what happened.

The executor owns one `AsyncMonty` session for the whole of a task attempt, because
Monty sessions persist globals across `feed_run` calls — that is what makes the
stateful follow-up task meaningful.

Everything the metrics need is captured here: which host functions were called and
when (for the round-trip metric), what was printed (fed back to the agent in the
agentic mode), how long the run took, and the structured error if it failed.
"""

from __future__ import annotations

import inspect
import json
import time
from collections.abc import Callable
from contextlib import AsyncExitStack
from dataclasses import dataclass, field
from types import TracebackType
from typing import Any, Self

from pydantic_monty import (
    AsyncMonty,
    AsyncMontySession,
    CollectStreams,
    MontyError,
    MontyRuntimeError,
    MontySyntaxError,
    MontyTypingError,
)

from .task import Task

__all__ = ('CallRecord', 'ExecutionOutcome', 'MontyExecutor', 'call_batches')


@dataclass(frozen=True)
class CallRecord:
    """One host-function invocation, with the wall-clock interval it occupied."""

    name: str
    started: float
    finished: float


def call_batches(calls: list[CallRecord]) -> int:
    """Count sequential waves of host calls — the round-trip proxy for the time axis.

    Calls whose intervals overlap were in flight together (an `asyncio.gather`), so
    they cost one round trip between them. Calls that strictly follow one another cost
    one round trip each. Twelve awaits in a loop score 12; the same twelve gathered
    score 1.

    This is a proxy, not a measurement: a host function fast enough that two sequential
    calls land inside the same clock tick would merge. Task 1 pins the expected value
    both ways so the proxy stays honest.
    """
    if not calls:
        return 0
    batches = 1
    ordered = sorted(calls, key=lambda c: c.started)
    batch_end = ordered[0].finished
    for call in ordered[1:]:
        if call.started >= batch_end:
            batches += 1
            batch_end = call.finished
        else:
            batch_end = max(batch_end, call.finished)
    return batches


@dataclass
class ExecutionOutcome:
    """Everything one `feed_run` produced, successful or not."""

    code: str
    result: Any = None
    error: MontyError | None = None
    stdout: str = ''
    stderr: str = ''
    calls: list[CallRecord] = field(default_factory=list)
    duration: float = 0.0

    @property
    def ok(self) -> bool:
        """True when the snippet ran to completion; the answer may still be wrong."""
        return self.error is None

    @property
    def external_calls(self) -> int:
        return len(self.calls)

    @property
    def call_batches(self) -> int:
        return call_batches(self.calls)

    @property
    def result_bytes(self) -> int:
        """Size of the returned value as JSON — the context-saving claim, measured.

        Values Monty returns that JSON cannot represent fall back to their `repr`,
        which is close enough for a size proxy.
        """
        try:
            return len(json.dumps(self.result, default=repr))
        except (TypeError, ValueError):
            return len(repr(self.result))

    def feedback(self) -> str:
        """Render this outcome as the next user turn for the agentic mode.

        Errors are rendered as a full Monty traceback because that is what a real
        integration would hand back — if the traceback is not enough for the model to
        self-correct, that is a finding about Monty's errors, not about the harness.
        """
        parts: list[str] = []
        if self.stdout:
            parts.append(f'Output printed while running:\n```\n{self.stdout.rstrip()}\n```')
        if self.error is not None:
            parts.append(
                f'The code failed:\n```\n{_render_error(self.error)}\n```\nFix it and return the corrected code.'
            )
        else:
            parts.append(f'The code ran and returned:\n```\n{self.result!r}\n```')
        return '\n\n'.join(parts)


def _render_error(error: MontyError) -> str:
    """Render an error the way a real integration would hand it back to the model.

    Runtime and syntax errors get the full traceback; type-check diagnostics come
    pre-rendered and their `display()` takes no format argument.
    """
    if isinstance(error, MontyTypingError):
        return error.display()
    if isinstance(error, (MontyRuntimeError, MontySyntaxError)):
        return error.display('traceback')
    return str(error)


class MontyExecutor:
    """One Monty session, held open across every turn of a single task attempt.

    Host functions are wrapped so each call is timed and counted. The wrappers
    preserve sync-vs-async: Monty awaits an async host function and calls a sync one
    directly, so a wrapper that changed the kind would change what the sandboxed code
    is allowed to write.
    """

    def __init__(self, task: Task) -> None:
        self._task = task
        self._stack = AsyncExitStack()
        self._session: AsyncMontySession | None = None
        self.calls: list[CallRecord] = []

    async def __aenter__(self) -> Self:
        pool = await self._stack.enter_async_context(AsyncMonty())
        self._session = await self._stack.enter_async_context(
            pool.checkout(
                script_name=f'{self._task.name}.py',
                limits=self._task.limits,
                type_check=True,
                type_check_stubs=self._task.stubs,
            )
        )
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self._stack.__aexit__(exc_type, exc, tb)

    async def feed(self, code: str) -> ExecutionOutcome:
        """Execute one snippet in the live session and record the outcome.

        Never raises for sandboxed failures — a `MontyError` is data here, since a
        model writing code Monty rejects is exactly what the suite measures. Only a
        harness bug propagates.
        """
        assert self._session is not None, 'MontyExecutor used outside its async context'
        streams = CollectStreams()
        before = len(self.calls)
        started = time.perf_counter()
        outcome = ExecutionOutcome(code=code)
        try:
            outcome.result = await self._session.feed_run(
                code,
                inputs=self._task.inputs or None,
                external_lookup=self._wrapped_tools(),
                print_callback=streams,
                mount=self._task.mounts or None,
            )
        except MontyError as exc:
            outcome.error = exc
        outcome.duration = time.perf_counter() - started
        outcome.stdout = ''.join(text for stream, text in streams.output if stream == 'stdout')
        outcome.stderr = ''.join(text for stream, text in streams.output if stream == 'stderr')
        outcome.calls = self.calls[before:]
        return outcome

    def _wrapped_tools(self) -> dict[str, Callable[..., Any]]:
        return {name: self._instrument(name, fn) for name, fn in self._task.tools.items()}

    def _instrument(self, name: str, fn: Callable[..., Any]) -> Callable[..., Any]:
        """Wrap a host function to record its call interval, preserving sync/async."""
        if inspect.iscoroutinefunction(fn):

            async def async_wrapper(*args: Any, **kwargs: Any) -> Any:
                started = time.perf_counter()
                try:
                    return await fn(*args, **kwargs)
                finally:
                    self.calls.append(CallRecord(name, started, time.perf_counter()))

            return async_wrapper

        def sync_wrapper(*args: Any, **kwargs: Any) -> Any:
            started = time.perf_counter()
            try:
                return fn(*args, **kwargs)
            finally:
                self.calls.append(CallRecord(name, started, time.perf_counter()))

        return sync_wrapper
