"""Task definitions for the Monty agent-code eval suite.

A `Task` is a realistic request an agent might be asked to satisfy by writing Python
that runs inside Monty, bundled with the host functions it may call, the stubs
describing them, and a way to decide whether the answer was right.

Tasks are deliberately chosen so that the *idiomatic* CPython solution crosses
surface Monty does not implement. That is what makes them useful: a construct no
model would reach for does not matter when it is missing.

Every task carries a `reference_solution` — Monty code known to produce the right
answer. `runner.py --dry-run` executes that instead of calling a model, so the whole
harness is testable with no API key and no spend.
"""

from __future__ import annotations

import math
from abc import ABC, abstractmethod
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from typing import Any, cast

from pydantic_monty import MountDir, ResourceLimits

__all__ = (
    'Approx',
    'CheckOutcome',
    'Every',
    'Exact',
    'Expectation',
    'Predicate',
    'Rubric',
    'Task',
    'Turn',
)


@dataclass(frozen=True)
class CheckOutcome:
    """Result of comparing a run's output against a task's expectation.

    `needs_judge` marks a check that no deterministic comparison can settle, so the
    runner must fall back to an LLM judge. It is distinct from `passed=False`: an
    unjudged rubric has not failed, it just has not been decided yet.
    """

    passed: bool
    detail: str
    needs_judge: bool = False


class Expectation(ABC):
    """How a task decides whether the agent's answer was correct."""

    @abstractmethod
    def check(self, result: Any) -> CheckOutcome:
        """Compare `result` — the value of the code's trailing expression — against this expectation."""


@dataclass(frozen=True)
class Exact(Expectation):
    """Exact structural equality. The default for anything with one right answer."""

    value: Any

    def check(self, result: Any) -> CheckOutcome:
        if result == self.value:
            return CheckOutcome(True, 'exact match')
        return CheckOutcome(False, f'expected {self.value!r}, got {result!r}')


@dataclass(frozen=True)
class Approx(Expectation):
    """Structural equality with a tolerance on floats, compared recursively.

    Use for anything with arithmetic in it: models legitimately differ on rounding
    order, and scoring that as a failure would drown the signal we actually want.
    """

    value: Any
    rel_tol: float = 1e-3
    abs_tol: float = 1e-9

    def check(self, result: Any) -> CheckOutcome:
        if self._close(result, self.value):
            return CheckOutcome(True, f'match within rel_tol={self.rel_tol}')
        return CheckOutcome(False, f'expected ~{self.value!r}, got {result!r}')

    def _close(self, a: Any, b: Any) -> bool:
        """Recurse through containers, comparing floats with a tolerance and everything else exactly."""
        if isinstance(a, bool) or isinstance(b, bool):
            return a is b
        if isinstance(a, (int, float)) and isinstance(b, (int, float)):
            return math.isclose(a, b, rel_tol=self.rel_tol, abs_tol=self.abs_tol)
        if isinstance(a, dict) and isinstance(b, dict):
            keys_a: set[Any] = set(a)  # pyright: ignore[reportUnknownArgumentType]
            keys_b: set[Any] = set(b)  # pyright: ignore[reportUnknownArgumentType]
            if keys_a != keys_b:
                return False
            return all(self._close(a[k], b[k]) for k in keys_a)
        if isinstance(a, (list, tuple)) and isinstance(b, (list, tuple)):
            seq_a = cast('Sequence[Any]', a)
            seq_b = cast('Sequence[Any]', b)
            if len(seq_a) != len(seq_b):
                return False
            return all(self._close(x, y) for x, y in zip(seq_a, seq_b))
        return bool(a == b)


@dataclass(frozen=True)
class Predicate(Expectation):
    """Arbitrary host-side validation, for answers no literal can express.

    Used where the check needs to parse the output (task 10 reads bar heights out of
    generated SVG) rather than compare it.
    """

    description: str
    fn: Callable[[Any], bool]

    def check(self, result: Any) -> CheckOutcome:
        try:
            passed = self.fn(result)
        except Exception as exc:  # noqa: BLE001 - a predicate blowing up is a failed check, not a crash
            return CheckOutcome(False, f'predicate {self.description!r} raised {exc!r}')
        return CheckOutcome(passed, f'predicate {self.description!r} {"passed" if passed else "failed"}')


@dataclass(frozen=True)
class Rubric(Expectation):
    """Defer to an LLM judge against a written rubric.

    Only for outputs with no machine-checkable ground truth (prose, chart legibility).
    Prefer a `Predicate` wherever the property can be parsed out instead — judges add
    variance, and variance is what this suite exists to remove.
    """

    rubric: str

    def check(self, result: Any) -> CheckOutcome:
        return CheckOutcome(False, 'awaiting judge', needs_judge=True)


@dataclass(frozen=True)
class Every(Expectation):
    """Conjunction — every sub-expectation must hold.

    Lets a task pair a deterministic floor with a judged rubric, e.g. "the SVG has
    correctly proportioned bars *and* a human would call it legible".
    """

    parts: tuple[Expectation, ...]

    def check(self, result: Any) -> CheckOutcome:
        details: list[str] = []
        needs_judge = False
        for part in self.parts:
            outcome = part.check(result)
            needs_judge = needs_judge or outcome.needs_judge
            if not outcome.passed and not outcome.needs_judge:
                return CheckOutcome(False, outcome.detail)
            details.append(outcome.detail)
        # `passed` reflects the deterministic parts alone. A caller with no judge
        # available keeps that verdict rather than failing the task outright, so a dry
        # run still proves everything machine-checkable about a rubric task.
        return CheckOutcome(True, '; '.join(details), needs_judge=needs_judge)


@dataclass(frozen=True)
class Turn:
    """One request in a multi-turn task.

    Multi-turn tasks exist to test whether the prompt correctly describes session
    state. Monty sessions *do* persist globals across `feed_run` calls, so a
    follow-up turn should reuse what turn 1 computed rather than re-fetching it —
    `expected_external_calls=0` on a later turn is how we score that.
    """

    prompt: str
    expected: Expectation
    reference_solution: str
    expected_external_calls: int | None = None


@dataclass
class Task:
    """A single scored task.

    `tools` are real callables: the runner wraps them to count invocations and record
    timing, which is where the external-call and round-trip metrics come from. `stubs`
    is hand-written `.pyi` text rather than `stubgen` output so tasks stay hermetic and
    reviewable.
    """

    name: str
    category: str
    prompt: str
    stubs: str
    tools: dict[str, Callable[..., Any]]
    expected: Expectation
    reference_solution: str
    traps: tuple[str, ...] = ()
    """Monty gaps the idiomatic solution is expected to cross — documentation, not enforced."""

    mounts: list[MountDir] = field(default_factory=list)
    inputs: dict[str, Any] = field(default_factory=dict)
    expected_external_calls: int | None = None
    expected_call_batches: int | None = None
    max_result_bytes: int | None = None
    limits: ResourceLimits | None = None
    follow_up: Turn | None = None
    """A second request against the same live session, for stateful tasks."""

    setup: Callable[[], None] | None = None
    """Called before each attempt. Required for any task whose tools keep state.

    `retry_flaky` counts attempts per record so a retry can succeed where the first try
    failed; without a reset, the second attempt of the task would see the first
    attempt's counters and score differently.
    """

    @property
    def qualified_name(self) -> str:
        """`category/name`, the identifier used on the command line and in reports."""
        return f'{self.category}/{self.name}'
