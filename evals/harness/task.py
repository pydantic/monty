"""Task definitions for the Monty agent-code eval suite.

A `Task` is a request an agent satisfies by writing Python that runs inside Monty,
bundled with the host functions it may call, the stubs describing them, and the
pydantic-evals evaluators that decide whether the answer was right. `Task.case()`
turns it into the `Case` the runner's `Dataset` is built from.

Every task carries a `reference_solution`, Monty code known to produce the right
answer, which `runner.py --dry-run` executes instead of calling a model.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

from pydantic_evals import Case
from pydantic_evals.evaluators import Evaluator, LLMJudge

from pydantic_monty import MountDir, ResourceLimits

__all__ = ('Task', 'Turn')


@dataclass(frozen=True)
class Turn:
    """A follow-up request against the same live session, for stateful tasks.

    `expected` is compared with the same tolerance as `ApproxExpected`, and
    `expected_external_calls` is what carries the signal: a follow-up that re-fetches
    gets the right answer and still fails.
    """

    prompt: str
    expected: Any
    reference_solution: str
    expected_external_calls: int | None = None


@dataclass
class Task:
    """A single scored task; `case()` is its pydantic-evals form.

    `tools` are real callables: the executor wraps them to count invocations and record
    timing. `stubs` is hand-written `.pyi` text so tasks stay hermetic and reviewable.
    `expected` becomes the case's `expected_output`; `evaluators` decide how it is
    compared (`EqualsExpected`, `ApproxExpected`, `Predicate`, `LLMJudge`).
    """

    name: str
    category: str
    prompt: str
    stubs: str
    tools: dict[str, Callable[..., Any]]
    reference_solution: str
    expected: Any = None
    evaluators: tuple[Evaluator[Task, Any, Any], ...] = ()
    traps: tuple[str, ...] = ()
    """Monty gaps the idiomatic solution is expected to cross; documentation, not enforced."""

    mounts: list[MountDir] = field(default_factory=list)
    inputs: dict[str, Any] = field(default_factory=dict)
    expected_external_calls: int | None = None
    expected_call_batches: int | None = None
    max_result_bytes: int | None = None
    limits: ResourceLimits | None = None
    follow_up: Turn | None = None
    setup: Callable[[], None] | None = None
    """Called before each attempt; required for any task whose tools keep state."""

    sub_model_stub: Callable[[str], str] | None = None
    """Declares a task with sub-model calls: the runner adds `llm_query(prompt)` and
    `call_llm(messages)` host functions.

    With a model both are backed by that model; under `--dry-run` this deterministic
    stand-in answers instead, so it must understand the reference solution's prompts.
    `call_llm` renders its messages as `role: text` lines before calling the stub.
    """

    expect_error: str | None = None
    """Exception name the primary request is expected to end with, e.g. `MemoryError`.

    The run then counts as having run, and the follow-up still goes ahead, so a task
    can check that the session survives a resource limit.
    """

    model_tools: dict[str, Callable[..., Any]] = field(default_factory=dict)
    """Tools only the model may call directly, never from sandbox code.

    They are registered on the pydantic-ai agent as ordinary tools. `--dry-run` has
    no model, so it replays `reference_model_tool_calls` after the reference code.
    """

    reference_model_tool_calls: tuple[tuple[str, dict[str, Any]], ...] = ()
    """`(tool name, kwargs)` pairs the dry run invokes in place of a model calling `model_tools`."""

    snapshot_at: str | None = None
    """Name of a sync host function at which the executor snapshots the run.

    On reaching the call the executor dumps the suspended interpreter, discards the
    session, restores the dump in a fresh one and resumes with the function's result,
    so the case proves the run survives moving between workers.
    """

    @property
    def qualified_name(self) -> str:
        """`category/name`, the identifier used on the command line and in reports."""
        return f'{self.category}/{self.name}'

    def case(self, *, judge: bool) -> Case[Task, Any, Any]:
        """Build the pydantic-evals case; `judge=False` drops `LLMJudge` evaluators."""
        evaluators = tuple(e for e in self.evaluators if judge or not isinstance(e, LLMJudge))
        return Case(
            name=self.qualified_name,
            inputs=self,
            metadata={'category': self.category, 'traps': list(self.traps)},
            expected_output=self.expected,
            evaluators=evaluators,
        )

    def __repr__(self) -> str:
        return f'Task({self.qualified_name!r})'
