"""Per-attempt metrics and the objective axes they roll up into.

There is no single "best" prompt, so there is no single score. A caller optimising for
latency and one optimising for token spend want different columns, and a prompt that
wins one usually loses another — telling the model to `gather` everything buys round
trips at the cost of readable code. Blending these into one number would hide exactly
the trade-off the suite exists to expose.
"""

from __future__ import annotations

import ast
from dataclasses import asdict, dataclass, field
from typing import Any

__all__ = ('AXES', 'AttemptMetrics', 'code_shape')

AXES = ('correctness', 'cost', 'time', 'simplicity')
"""The objective axes reported separately in the scoreboard."""


@dataclass
class AttemptMetrics:
    """Everything measured about one task attempt under one prompt and one model."""

    task: str
    prompt_variant: str
    model: str
    mode: str
    repeat: int = 0

    success: bool = False
    first_attempt_runs: bool = False
    type_check_passed: bool = True
    turns_used: int = 0
    detail: str = ''

    prompt_tokens: int = 0
    completion_tokens: int = 0
    result_bytes: int = 0

    external_calls: int = 0
    call_batches: int = 0
    expected_external_calls: int | None = None
    expected_call_batches: int | None = None
    duration: float = 0.0

    code_lines: int = 0
    max_nesting: int = 0

    judge_score: float | None = None
    judge_reason: str = ''

    gaps: list[dict[str, Any]] = field(default_factory=list)
    """Feature gaps hit during this attempt, as serialised `FeatureGap` records."""

    @property
    def total_tokens(self) -> int:
        return self.prompt_tokens + self.completion_tokens

    @property
    def calls_as_expected(self) -> bool | None:
        """Whether the run made the number of host calls the task expects.

        `None` when the task does not pin a count. A wrong count with a right answer is
        still a finding: it usually means the model fetched more than it needed.
        """
        if self.expected_external_calls is None:
            return None
        return self.external_calls == self.expected_external_calls

    def as_row(self) -> dict[str, Any]:
        """Flatten for JSON output and report aggregation."""
        row = asdict(self)
        row['total_tokens'] = self.total_tokens
        row['calls_as_expected'] = self.calls_as_expected
        return row


def code_shape(code: str) -> tuple[int, int]:
    """Return `(non-blank lines, maximum block nesting depth)` for the simplicity axis.

    Nesting depth is counted over the constructs that actually indent a block, so a long
    flat script scores better than a short deeply-nested one. Unparseable code scores
    its line count with zero depth rather than raising — the model emitting code Monty
    cannot parse is already recorded as a failure elsewhere.
    """
    lines = len([line for line in code.splitlines() if line.strip()])
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return lines, 0
    return lines, _depth(tree)


_NESTING_NODES = (
    ast.For,
    ast.AsyncFor,
    ast.While,
    ast.If,
    ast.With,
    ast.AsyncWith,
    ast.Try,
    ast.FunctionDef,
    ast.AsyncFunctionDef,
    ast.ClassDef,
)


def _depth(node: ast.AST, current: int = 0) -> int:
    """Deepest chain of block-introducing nodes below `node`."""
    deepest = current
    for child in ast.iter_child_nodes(node):
        child_depth = current + 1 if isinstance(child, _NESTING_NODES) else current
        deepest = max(deepest, _depth(child, child_depth))
    return deepest
