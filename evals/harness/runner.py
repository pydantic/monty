"""Drives tasks through a prompt variant and a model, and scores what comes back.

Two modes, and running both is the point:

- **single** — one code block, executed, graded. Cheap and low-variance, so it measures
  what the *prompt* achieved.
- **agentic** — errors and printed output feed back as the next user turn, up to a cap.
  It measures what error feedback can repair.

A prompt that only wins in agentic mode is buying turns, not quality — which is why the
scoreboard reports the two separately rather than averaging them.
"""

from __future__ import annotations

import argparse
import asyncio
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

from pydantic_monty import MontyTypingError

from .agent import CodeAgent, DryRunAgent, Reply, load_prompt
from .classify import classify
from .executor import ExecutionOutcome, MontyExecutor
from .judge import judge_result
from .metrics import AttemptMetrics, code_shape
from .registry import all_tasks, load_task
from .report import write_reports
from .task import Task, Turn

__all__ = ('main', 'run_attempt')

DEFAULT_MAX_TURNS = 4
REPORTS_DIR = Path(__file__).parent.parent / 'reports'


class _Agent(Protocol):
    """The slice of an agent the runner needs — satisfied by `CodeAgent` and `DryRunAgent`."""

    async def respond(self, user_text: str) -> Reply: ...


@dataclass
class _TurnResult:
    """Outcome of driving one request to completion (or to the turn cap)."""

    outcome: ExecutionOutcome | None
    turns: int
    first_attempt_ran: bool
    type_check_passed: bool
    prompt_tokens: int
    completion_tokens: int
    gaps: list[dict[str, Any]]
    last_code: str


async def run_attempt(
    task: Task,
    *,
    prompt_variant: str,
    model: str,
    mode: str,
    repeat: int = 0,
    dry_run: bool = False,
    max_turns: int = DEFAULT_MAX_TURNS,
    judge_model: str | None = None,
) -> AttemptMetrics:
    """Run one task once and return its metrics.

    The Monty session stays open across the task's follow-up turn so the stateful task
    can check that a follow-up reuses session state instead of re-fetching.
    """
    if task.setup is not None:
        task.setup()
    system_prompt = load_prompt(prompt_variant).replace('{stubs}', task.stubs.strip())
    agent: _Agent = (
        DryRunAgent([task.reference_solution] + ([task.follow_up.reference_solution] if task.follow_up else []))
        if dry_run
        else CodeAgent(model=model, system_prompt=system_prompt)
    )
    turn_cap = 1 if (mode == 'single' or dry_run) else max_turns

    metrics = AttemptMetrics(
        task=task.qualified_name,
        prompt_variant=prompt_variant,
        model='reference' if dry_run else model,
        mode=mode,
        repeat=repeat,
        expected_external_calls=task.expected_external_calls,
        expected_call_batches=task.expected_call_batches,
    )

    async with MontyExecutor(task) as executor:
        primary = await _drive(agent, executor, task.prompt, turn_cap)
        _absorb(metrics, primary)

        outcome = primary.outcome
        if outcome is None:
            metrics.detail = 'model returned no code'
            return metrics

        check = task.expected.check(outcome.result)
        metrics.success = check.passed
        metrics.detail = check.detail
        metrics.external_calls = outcome.external_calls
        metrics.call_batches = outcome.call_batches
        metrics.result_bytes = outcome.result_bytes
        metrics.duration = outcome.duration

        if check.needs_judge:
            await _apply_judge(metrics, task, outcome, judge_model)

        if task.follow_up is not None and metrics.success:
            await _run_follow_up(metrics, task.follow_up, agent, executor, turn_cap)

    return metrics


async def _drive(
    agent: _Agent,
    executor: MontyExecutor,
    request: str,
    turn_cap: int,
) -> _TurnResult:
    """Ask for code, run it, and on failure hand the error back until the cap is hit."""
    prompt_tokens = completion_tokens = 0
    gaps: list[dict[str, Any]] = []
    first_attempt_ran = False
    type_check_passed = True
    outcome: ExecutionOutcome | None = None
    user_text = request
    turns = 0

    for turn in range(turn_cap):
        reply = await agent.respond(user_text)
        prompt_tokens += reply.prompt_tokens
        completion_tokens += reply.completion_tokens
        if reply.code is None:
            break
        turns = turn + 1
        outcome = await executor.feed(reply.code)
        if turn == 0:
            first_attempt_ran = outcome.ok
        if outcome.error is not None:
            if isinstance(outcome.error, MontyTypingError):
                type_check_passed = False
            gap = classify(outcome.error)
            if gap is not None:
                gaps.append(
                    {
                        'kind': gap.kind,
                        'symbol': gap.symbol,
                        'message': gap.message,
                        'source_line': gap.source_line,
                        'certain': gap.certain,
                        'doc': gap.doc,
                    }
                )
            user_text = outcome.feedback()
            continue
        break

    last_code = outcome.code if outcome is not None else ''
    return _TurnResult(
        outcome=outcome,
        turns=turns,
        first_attempt_ran=first_attempt_ran,
        type_check_passed=type_check_passed,
        prompt_tokens=prompt_tokens,
        completion_tokens=completion_tokens,
        gaps=gaps,
        last_code=last_code,
    )


def _absorb(metrics: AttemptMetrics, result: _TurnResult) -> None:
    """Fold a driven request's counters into the attempt's metrics."""
    metrics.turns_used += result.turns
    metrics.prompt_tokens += result.prompt_tokens
    metrics.completion_tokens += result.completion_tokens
    metrics.gaps.extend(result.gaps)
    metrics.first_attempt_runs = result.first_attempt_ran
    metrics.type_check_passed = metrics.type_check_passed and result.type_check_passed
    if result.last_code:
        metrics.code_lines, metrics.max_nesting = code_shape(result.last_code)


async def _run_follow_up(
    metrics: AttemptMetrics,
    follow_up: Turn,
    agent: _Agent,
    executor: MontyExecutor,
    turn_cap: int,
) -> None:
    """Score the second request of a stateful task against the same live session.

    The follow-up's own external-call expectation is what carries the signal: a correct
    answer that re-fetched everything means the prompt failed to convey that session
    state persists, even though the answer is right.
    """
    result = await _drive(agent, executor, follow_up.prompt, turn_cap)
    _absorb(metrics, result)
    if result.outcome is None:
        metrics.success = False
        metrics.detail = 'follow-up returned no code'
        return

    check = follow_up.expected.check(result.outcome.result)
    metrics.detail = f'{metrics.detail}; follow-up: {check.detail}'
    metrics.success = check.passed
    if follow_up.expected_external_calls is not None:
        actual = result.outcome.external_calls
        if actual != follow_up.expected_external_calls:
            metrics.success = False
            metrics.detail += f'; follow-up made {actual} host calls, expected {follow_up.expected_external_calls}'


async def _apply_judge(
    metrics: AttemptMetrics,
    task: Task,
    outcome: ExecutionOutcome,
    judge_model: str | None,
) -> None:
    """Settle a rubric-based expectation with an LLM judge."""
    if judge_model is None:
        # Keep the deterministic verdict rather than failing: a rubric task's
        # machine-checkable half is still worth scoring, and a dry run has no judge.
        metrics.detail = f'{metrics.detail}; rubric skipped (no --judge-model)'
        return
    verdict = await judge_result(task, outcome.result, judge_model)
    metrics.judge_score = verdict.score
    metrics.judge_reason = verdict.reason
    metrics.success = verdict.passed
    metrics.detail = f'{metrics.detail}; judge: {verdict.reason}'


async def _run_all(args: argparse.Namespace) -> list[AttemptMetrics]:
    """Expand the requested tasks × prompts × modes × repeats and run them."""
    tasks = [load_task(name) for name in args.task] if args.task else all_tasks()
    modes = ['single', 'agentic'] if args.mode == 'both' else [args.mode]
    results: list[AttemptMetrics] = []

    for task in tasks:
        for variant in args.prompt:
            for mode in modes:
                for repeat in range(args.repeat):
                    metrics = await run_attempt(
                        task,
                        prompt_variant=variant,
                        model=args.model,
                        mode=mode,
                        repeat=repeat,
                        dry_run=args.dry_run,
                        max_turns=args.max_turns,
                        judge_model=args.judge_model,
                    )
                    results.append(metrics)
                    status = 'PASS' if metrics.success else 'FAIL'
                    print(f'{status}  {task.qualified_name:34} {variant:14} {mode:8} {metrics.detail[:70]}')
    return results


def main(argv: list[str] | None = None) -> int:
    """CLI entry point. Returns a non-zero exit status when any attempt failed."""
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--task', action='append', default=[], help='task to run, e.g. numeric/expense_budget')
    parser.add_argument('--all', action='store_true', help='run every task (the default when --task is absent)')
    parser.add_argument('--prompt', default='v4_codemode', help='comma-separated prompt variants')
    parser.add_argument('--model', default='anthropic:claude-sonnet-4-5', help='model to generate code with')
    parser.add_argument('--judge-model', default=None, help='model for rubric expectations; omit to skip them')
    parser.add_argument('--mode', choices=['single', 'agentic', 'both'], default='single')
    parser.add_argument('--repeat', type=int, default=1, help='attempts per combination, for variance')
    parser.add_argument('--max-turns', type=int, default=DEFAULT_MAX_TURNS)
    parser.add_argument(
        '--dry-run',
        action='store_true',
        help="execute each task's reference solution instead of calling a model",
    )
    parser.add_argument('--reports', type=Path, default=REPORTS_DIR)
    args = parser.parse_args(argv)
    args.prompt = [variant for variant in args.prompt.split(',') if variant]

    results = asyncio.run(_run_all(args))
    write_reports(results, args.reports)

    failed = [metrics for metrics in results if not metrics.success]
    print(f'\n{len(results) - len(failed)}/{len(results)} passed. Reports written to {args.reports}')
    if failed and args.dry_run:
        print('\nA failing reference solution means the task is wrong or Monty has a real gap:')
        for metrics in failed:
            print(f'  {metrics.task}: {metrics.detail}')
            print(json.dumps(metrics.gaps, indent=2) if metrics.gaps else '  (no feature gap recorded)')
    return 1 if failed else 0


if __name__ == '__main__':
    raise SystemExit(main())
