"""Names of the metrics and attributes the solver records on each pydantic-evals case.

The solver records them with `increment_eval_metric` / `set_eval_attribute`; the
evaluators and `report.py` read them back from `ctx.metrics` / `ctx.attributes`.
Keeping the names here stops the two sides drifting apart.
"""

from __future__ import annotations

import ast

__all__ = ('ATTR', 'METRIC', 'code_shape')


class METRIC:
    """Numeric per-case metrics, all recorded via `increment_eval_metric`."""

    PROMPT_TOKENS = 'prompt_tokens'
    COMPLETION_TOKENS = 'completion_tokens'
    TURNS = 'turns'
    EXTERNAL_CALLS = 'external_calls'
    CALL_BATCHES = 'call_batches'
    RESULT_BYTES = 'result_bytes'
    CODE_LINES = 'code_lines'
    MAX_NESTING = 'max_nesting'
    FOLLOW_UP_EXTERNAL_CALLS = 'follow_up_external_calls'
    SNAPSHOTS = 'snapshots'


class ATTR:
    """Non-numeric per-case attributes, recorded via `set_eval_attribute`."""

    CODE = 'code'
    ERROR = 'error'
    FIRST_ATTEMPT_RUNS = 'first_attempt_runs'
    TYPE_CHECK_PASSED = 'type_check_passed'
    GAPS = 'gaps'
    FOLLOW_UP_RESULT = 'follow_up_result'
    FOLLOW_UP_ERROR = 'follow_up_error'


def code_shape(code: str) -> tuple[int, int]:
    """Return `(non-blank lines, maximum block nesting depth)` for the simplicity axis.

    Nesting is counted over the constructs that indent a block, so a long flat script
    scores better than a short deeply-nested one. Unparsable code scores its line
    count with zero depth; the parse failure is already recorded elsewhere.
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
