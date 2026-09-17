"""pydantic-evals evaluators for the suite.

Case-level evaluators (`ApproxExpected`, `Predicate`, plus pydantic-evals' own
`EqualsExpected` and `LLMJudge`) decide whether the answer was right. The dataset-level
ones in `DATASET_EVALUATORS` read the metrics and attributes the solver recorded and
turn the task's pinned expectations (call count, round trips, result size, follow-up)
into assertions; each returns `{}` when the task pins nothing, so it is skipped.
"""

from __future__ import annotations

import math
import re
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Any, cast

from pydantic_evals.evaluators import EvaluationReason, Evaluator, EvaluatorContext

from .metrics import ATTR, METRIC
from .task import Task

__all__ = (
    'DATASET_EVALUATORS',
    'ApproxExpected',
    'CallsAsExpected',
    'FollowUp',
    'Predicate',
    'ResultSize',
    'RunOutcome',
    'WithinCallBudget',
)

REL_TOL = 1e-3
ABS_TOL = 1e-9


@dataclass
class ApproxExpected(Evaluator[Task, Any, Any]):
    """`EqualsExpected` with a tolerance on floats, compared recursively.

    Use for anything with arithmetic in it: models legitimately differ on rounding
    order, and scoring that as a failure would drown the signal.
    """

    rel_tol: float = REL_TOL
    abs_tol: float = ABS_TOL

    def evaluate(self, ctx: EvaluatorContext[Task, Any, Any]) -> EvaluationReason:
        if approx_equal(ctx.output, ctx.expected_output, self.rel_tol, self.abs_tol):
            return EvaluationReason(True)
        return EvaluationReason(False, f'expected ~{ctx.expected_output!r}, got {ctx.output!r}')


@dataclass
class Predicate(Evaluator[Task, Any, Any]):
    """Host-side validation for answers no literal can express, e.g. parsing an SVG."""

    description: str
    fn: Callable[[Any], bool]

    def evaluate(self, ctx: EvaluatorContext[Task, Any, Any]) -> EvaluationReason:
        try:
            passed = self.fn(ctx.output)
        except Exception as exc:
            return EvaluationReason(False, f'predicate raised {exc!r}')
        return EvaluationReason(passed, self.description)

    def get_default_evaluation_name(self) -> str:
        # Named from the description so two predicates on one case get separate columns.
        return 'predicate_' + re.sub(r'[^a-z0-9]+', '_', self.description.lower()).strip('_')[:40]


@dataclass
class RunOutcome(Evaluator[Task, Any, Any]):
    """Assertions from the solver's attributes: did the first code block run, and did it type-check."""

    def evaluate(self, ctx: EvaluatorContext[Task, Any, Any]) -> dict[str, EvaluationReason]:
        error = ctx.attributes.get(ATTR.ERROR)
        # An error the task asked for is the run doing its job, not failing to run.
        ran = bool(ctx.attributes.get(ATTR.FIRST_ATTEMPT_RUNS)) or expected_error_hit(ctx.inputs, error)
        out = {
            ATTR.FIRST_ATTEMPT_RUNS: EvaluationReason(ran, error),
            ATTR.TYPE_CHECK_PASSED: EvaluationReason(bool(ctx.attributes.get(ATTR.TYPE_CHECK_PASSED, True))),
        }
        if ctx.inputs.expect_error is not None:
            # The task wants a particular failure; getting it is the pass condition.
            out['expected_error'] = EvaluationReason(expected_error_hit(ctx.inputs, error), error)
        return out


@dataclass
class CallsAsExpected(Evaluator[Task, Any, Any]):
    """The run made exactly the number of host calls the task pins.

    A wrong count with a right answer usually means the model fetched more than it
    needed.
    """

    def evaluate(self, ctx: EvaluatorContext[Task, Any, Any]) -> dict[str, EvaluationReason]:
        expected = ctx.inputs.expected_external_calls
        if expected is None:
            return {}
        actual = ctx.metrics.get(METRIC.EXTERNAL_CALLS, 0)
        return {'calls_as_expected': EvaluationReason(actual == expected, f'{actual} calls, expected {expected}')}


@dataclass
class WithinCallBudget(Evaluator[Task, Any, Any]):
    """The run made no more sequential waves of host calls than the task allows: the time axis."""

    def evaluate(self, ctx: EvaluatorContext[Task, Any, Any]) -> dict[str, EvaluationReason]:
        budget = ctx.inputs.expected_call_batches
        if budget is None:
            return {}
        actual = ctx.metrics.get(METRIC.CALL_BATCHES, 0)
        return {'within_call_budget': EvaluationReason(actual <= budget, f'{actual} round trips, budget {budget}')}


@dataclass
class ResultSize(Evaluator[Task, Any, Any]):
    """The returned value serialises under `max_result_bytes`: the context-saving claim, measured."""

    def evaluate(self, ctx: EvaluatorContext[Task, Any, Any]) -> dict[str, EvaluationReason]:
        limit = ctx.inputs.max_result_bytes
        if limit is None:
            return {}
        actual = ctx.metrics.get(METRIC.RESULT_BYTES, 0)
        return {'result_size': EvaluationReason(actual < limit, f'{actual} bytes, limit {limit}')}


@dataclass
class FollowUp(Evaluator[Task, Any, Any]):
    """Scores a stateful task's second turn from the attributes the solver recorded."""

    def evaluate(self, ctx: EvaluatorContext[Task, Any, Any]) -> dict[str, EvaluationReason]:
        follow_up = ctx.inputs.follow_up
        if follow_up is None:
            return {}
        result = ctx.attributes.get(ATTR.FOLLOW_UP_RESULT)
        error = ctx.attributes.get(ATTR.FOLLOW_UP_ERROR)
        correct = error is None and approx_equal(result, follow_up.expected, REL_TOL, ABS_TOL)
        out = {
            'follow_up_correct': EvaluationReason(correct, error or f'expected ~{follow_up.expected!r}, got {result!r}')
        }
        if follow_up.expected_external_calls is not None:
            actual = ctx.metrics.get(METRIC.FOLLOW_UP_EXTERNAL_CALLS, 0)
            out['follow_up_calls_as_expected'] = EvaluationReason(
                actual == follow_up.expected_external_calls,
                f'{actual} calls, expected {follow_up.expected_external_calls}',
            )
        return out


DATASET_EVALUATORS: tuple[Evaluator[Task, Any, Any], ...] = (
    RunOutcome(),
    CallsAsExpected(),
    WithinCallBudget(),
    ResultSize(),
    FollowUp(),
)
"""Applied to every case; each skips itself when the task pins nothing for it."""


def expected_error_hit(task: Task, error: object) -> bool:
    """Whether `error` (the rendered `Type: message` line) is the failure `task.expect_error` names."""
    return task.expect_error is not None and isinstance(error, str) and error.startswith(f'{task.expect_error}:')


def approx_equal(a: Any, b: Any, rel_tol: float, abs_tol: float) -> bool:
    """Recurse through containers, comparing floats with a tolerance and everything else exactly."""
    if isinstance(a, bool) or isinstance(b, bool):
        return a is b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return math.isclose(a, b, rel_tol=rel_tol, abs_tol=abs_tol)
    if isinstance(a, dict) and isinstance(b, dict):
        keys_a: set[Any] = set(a)  # pyright: ignore[reportUnknownArgumentType]
        keys_b: set[Any] = set(b)  # pyright: ignore[reportUnknownArgumentType]
        if keys_a != keys_b:
            return False
        return all(approx_equal(a[k], b[k], rel_tol, abs_tol) for k in keys_a)
    if isinstance(a, (list, tuple)) and isinstance(b, (list, tuple)):
        seq_a = cast('Sequence[Any]', a)
        seq_b = cast('Sequence[Any]', b)
        if len(seq_a) != len(seq_b):
            return False
        return all(approx_equal(x, y, rel_tol, abs_tol) for x, y in zip(seq_a, seq_b))
    return bool(a == b)
