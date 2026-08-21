"""LLM judging, for the few outputs no deterministic check can settle.

Deliberately narrow. A judge adds variance, and variance is what this suite exists to
remove — so a task should reach for `Predicate` and parse the property out of the
output wherever that is possible, and use `Rubric` only when the quality being judged
is genuinely subjective (is this chart legible, is this summary faithful).

Like `agent.py`, `pydantic_ai` is imported at call time so the rest of the harness —
and `--dry-run` in particular — works without the LLM stack.
"""

from __future__ import annotations

from dataclasses import dataclass

from .task import Rubric, Task

__all__ = ('Verdict', 'judge_result')


@dataclass(frozen=True)
class Verdict:
    """A judge's decision about one output."""

    score: float
    passed: bool
    reason: str


async def judge_result(task: Task, result: object, judge_model: str) -> Verdict:
    """Score `result` against the task's rubric.

    The judge sees the rubric and the output but *not* the task's prompt history or the
    code that produced it: it is grading the artefact, not the reasoning, and giving it
    the transcript would let a confident explanation paper over a bad result.
    """
    from pydantic import BaseModel, Field
    from pydantic_ai import Agent

    class _JudgeOutput(BaseModel):
        """Structured verdict, so the score is a number rather than parsed prose."""

        score: float = Field(ge=0.0, le=1.0, description='How well the output meets the rubric, 0 to 1.')
        passes: bool = Field(description='Whether the output meets the rubric well enough to count as correct.')
        reason: str = Field(description='One sentence explaining the score.')

    rubric = _rubric_of(task)
    if rubric is None:
        raise ValueError(f'{task.qualified_name} has no Rubric to judge')

    agent = Agent(
        judge_model,
        output_type=_JudgeOutput,
        instructions=(
            'You are grading the output of a program against a rubric. '
            'Judge only what the rubric asks about. Be strict: a plausible-looking '
            'output that does not satisfy the rubric fails.'
        ),
    )
    response = await agent.run(f'Rubric:\n{rubric}\n\nProgram output:\n{result!r}')
    verdict = response.output
    return Verdict(score=verdict.score, passed=verdict.passes, reason=verdict.reason)


def _rubric_of(task: Task) -> str | None:
    """Find the task's rubric, whether it stands alone or sits inside an `Every`."""
    expectation = task.expected
    if isinstance(expectation, Rubric):
        return expectation.rubric
    parts = getattr(expectation, 'parts', ())
    for part in parts:
        if isinstance(part, Rubric):
            return part.rubric
    return None
