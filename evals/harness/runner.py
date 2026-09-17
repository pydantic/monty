"""Runs the task suite as pydantic-evals experiments, one per prompt × model × mode.

`Solver.solve` is the pydantic-evals task function: it asks the model for code, runs it
in Monty, records what happened as metrics and attributes, and returns the value of
the code's trailing expression. The evaluators in `evaluators.py` then score it.

Three modes:

- `single`: one code block, executed, scored. Measures the prompt.
- `agentic`: Monty's error and printed output go back to the model as the next turn,
  up to `--max-turns`. Measures what error feedback repairs.
- `repl`: every outcome goes back, success included, until the model replies without
  code or the cap is hit. The RLM interaction pattern: peek, then decide what to run.
"""

from __future__ import annotations

import argparse
import asyncio
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol

from pydantic_evals import Dataset, set_eval_attribute
from pydantic_evals.dataset import increment_eval_metric
from pydantic_evals.evaluators.llm_as_a_judge import set_default_judge_model
from pydantic_evals.lifecycle import CaseLifecycle
from pydantic_evals.reporting import EvaluationReport

from pydantic_monty import MontyTypingError

from .agent import CodeAgent, DryRunAgent, Reply, SubModel, load_prompt, render_messages
from .classify import classify
from .evaluators import DATASET_EVALUATORS, expected_error_hit
from .executor import ExecutionOutcome, MontyExecutor
from .metrics import ATTR, METRIC, code_shape
from .registry import all_tasks, load_task
from .report import write_reports
from .task import Task

__all__ = ('Solver', 'build_dataset', 'main')

DEFAULT_MAX_TURNS = 4
MODES = ('single', 'agentic', 'repl')
STUB_LATENCY = 0.005
"""Seconds a dry-run `llm_query` sleeps, so gathered sub-calls overlap and `call_batches` can see them."""
REPORTS_DIR = Path(__file__).parent.parent / 'reports'


def build_dataset(tasks: list[Task], *, judge: bool) -> Dataset[Task, Any, Any]:
    """One case per task; `judge=False` drops `LLMJudge` evaluators so no judge model is needed."""
    return Dataset(
        name='monty-agent-code',
        cases=[task.case(judge=judge) for task in tasks],
        evaluators=DATASET_EVALUATORS,
    )


class _Agent(Protocol):
    """The slice of an agent the solver needs; satisfied by `CodeAgent` and `DryRunAgent`."""

    async def respond(self, user_text: str) -> Reply: ...


@dataclass(frozen=True)
class Solver:
    """One prompt × model × mode combination; `solve` is the pydantic-evals task function."""

    prompt_variant: str
    model: str
    mode: str
    dry_run: bool = False
    max_turns: int = DEFAULT_MAX_TURNS

    @property
    def experiment_name(self) -> str:
        return f'{self.prompt_variant}/{self.model_label}/{self.mode}'

    @property
    def model_label(self) -> str:
        return 'reference' if self.dry_run else self.model

    def metadata(self) -> dict[str, str]:
        """Experiment metadata, which `report.py` reads back to label its rows."""
        return {'prompt': self.prompt_variant, 'model': self.model_label, 'mode': self.mode}

    async def solve(self, task: Task) -> Any:
        """Drive one task attempt and return the primary result, or `None` if nothing ran.

        The Monty session stays open across the task's follow-up turn so a stateful
        task can check that the follow-up reuses session state instead of re-fetching.
        """
        system_prompt = load_prompt(self.prompt_variant).replace('{stubs}', task.stubs.strip())
        agent: _Agent = (
            DryRunAgent([task.reference_solution] + ([task.follow_up.reference_solution] if task.follow_up else []))
            if self.dry_run
            else CodeAgent(model=self.model, system_prompt=system_prompt, tools=task.model_tools)
        )
        turn_cap = 1 if (self.mode == 'single' or self.dry_run) else self.max_turns
        repl = self.mode == 'repl'
        sub_model = None if self.dry_run or task.sub_model_stub is None else SubModel(self.model)
        extra_tools = _llm_tools(task, sub_model)

        async with MontyExecutor(task, extra_tools, snapshot_at=task.snapshot_at) as executor:
            primary = await _drive(agent, executor, task.prompt, turn_cap, repl=repl)
            set_eval_attribute(ATTR.FIRST_ATTEMPT_RUNS, primary.first_attempt_ran)
            set_eval_attribute(ATTR.TYPE_CHECK_PASSED, primary.type_check_passed)
            gaps = list(primary.gaps)
            outcome = primary.outcome
            if outcome is None:
                set_eval_attribute(ATTR.ERROR, 'model returned no code')
                set_eval_attribute(ATTR.GAPS, gaps)
                return None

            lines, nesting = code_shape(outcome.code)
            increment_eval_metric(METRIC.CODE_LINES, lines)
            increment_eval_metric(METRIC.MAX_NESTING, nesting)
            increment_eval_metric(METRIC.EXTERNAL_CALLS, outcome.external_calls)
            increment_eval_metric(METRIC.CALL_BATCHES, outcome.call_batches)
            increment_eval_metric(METRIC.RESULT_BYTES, outcome.result_bytes)
            increment_eval_metric(METRIC.SNAPSHOTS, executor.snapshots)
            set_eval_attribute(ATTR.CODE, outcome.code)
            set_eval_attribute(ATTR.ERROR, outcome.error_message)
            if self.dry_run:
                # No model to call `model_tools`, so replay the calls the reference relies on.
                for name, kwargs in task.reference_model_tool_calls:
                    task.model_tools[name](**kwargs)

            if task.follow_up is not None and (outcome.ok or expected_error_hit(task, outcome.error_message)):
                follow_up = await _drive(agent, executor, task.follow_up.prompt, turn_cap, repl=repl)
                gaps += follow_up.gaps
                if follow_up.outcome is None:
                    set_eval_attribute(ATTR.FOLLOW_UP_ERROR, 'model returned no code')
                else:
                    set_eval_attribute(ATTR.FOLLOW_UP_RESULT, follow_up.outcome.result)
                    set_eval_attribute(ATTR.FOLLOW_UP_ERROR, follow_up.outcome.error_message)
                    increment_eval_metric(METRIC.FOLLOW_UP_EXTERNAL_CALLS, follow_up.outcome.external_calls)

        if sub_model is not None:
            increment_eval_metric(METRIC.PROMPT_TOKENS, sub_model.prompt_tokens)
            increment_eval_metric(METRIC.COMPLETION_TOKENS, sub_model.completion_tokens)
        set_eval_attribute(ATTR.GAPS, gaps)
        return outcome.result


def _llm_tools(task: Task, sub_model: SubModel | None) -> dict[str, Any]:
    """The `llm_query` and `call_llm` host functions for a sub-model task, or nothing for the rest."""
    if task.sub_model_stub is None:
        return {}
    if sub_model is None:
        stub = task.sub_model_stub

        async def llm_query(prompt: str) -> str:
            await asyncio.sleep(STUB_LATENCY)
            return stub(prompt)

        async def call_llm(messages: list[dict[str, str]]) -> str:
            await asyncio.sleep(STUB_LATENCY)
            return stub(render_messages(messages))

        return {'llm_query': llm_query, 'call_llm': call_llm}
    return {'llm_query': sub_model.llm_query, 'call_llm': sub_model.call_llm}


@dataclass
class _Driven:
    """What `_drive` learned from one request: the last outcome plus per-turn findings."""

    outcome: ExecutionOutcome | None = None
    first_attempt_ran: bool = False
    type_check_passed: bool = True
    gaps: list[dict[str, Any]] = field(default_factory=list)


async def _drive(agent: _Agent, executor: MontyExecutor, request: str, turn_cap: int, *, repl: bool) -> _Driven:
    """Ask for code, run it, and hand the outcome back until the cap is hit.

    Failures always go back; with `repl` successes do too, and the last executed
    result stands when the model stops writing code. Token and turn counts are
    recorded here because they accumulate across the primary request and the follow-up.
    """
    driven = _Driven()
    user_text = request
    for turn in range(turn_cap):
        reply = await agent.respond(user_text)
        increment_eval_metric(METRIC.PROMPT_TOKENS, reply.prompt_tokens)
        increment_eval_metric(METRIC.COMPLETION_TOKENS, reply.completion_tokens)
        if reply.code is None:
            break
        increment_eval_metric(METRIC.TURNS, 1)
        outcome = await executor.feed(reply.code)
        driven.outcome = outcome
        if turn == 0:
            driven.first_attempt_ran = outcome.ok
        if outcome.error is None and not repl:
            break
        if outcome.error is not None:
            if isinstance(outcome.error, MontyTypingError):
                driven.type_check_passed = False
            gap = classify(outcome.error)
            if gap is not None:
                driven.gaps.append(gap.as_dict())
        user_text = outcome.feedback(repl=repl)
    return driven


class _TaskLifecycle(CaseLifecycle[Task, Any, Any]):
    """Runs the task's `setup` before each attempt, so stateful tools start clean."""

    async def setup(self) -> None:
        if self.case.inputs.setup is not None:
            self.case.inputs.setup()


async def _run_all(args: argparse.Namespace) -> list[EvaluationReport[Task, Any, Any]]:
    """Run one experiment per prompt × mode and return their reports."""
    tasks = [load_task(name) for name in args.task] if args.task else all_tasks()
    if args.judge_model is not None:
        set_default_judge_model(args.judge_model)
    dataset = build_dataset(tasks, judge=args.judge_model is not None)
    modes = list(MODES) if args.mode == 'all' else [args.mode]

    reports: list[EvaluationReport[Task, Any, Any]] = []
    for variant in args.prompt:
        for mode in modes:
            solver = Solver(variant, args.model, mode, dry_run=args.dry_run, max_turns=args.max_turns)
            report = await dataset.evaluate(
                solver.solve,
                name=solver.experiment_name,
                metadata=solver.metadata(),
                repeat=args.repeat,
                max_concurrency=args.concurrency,
                lifecycle=_TaskLifecycle,
            )
            report.print(include_reasons=True, include_output=False)
            reports.append(report)
    return reports


def main(argv: list[str] | None = None) -> int:
    """CLI entry point. Returns a non-zero exit status when any case failed an assertion."""
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--task', action='append', default=[], help='task to run, e.g. numeric/expense_budget')
    parser.add_argument('--all', action='store_true', help='run every task (the default when --task is absent)')
    parser.add_argument('--prompt', default='v4_codemode', help='comma-separated prompt variants')
    parser.add_argument('--model', default='anthropic:claude-sonnet-4-5', help='model to generate code with')
    parser.add_argument('--judge-model', default=None, help='model for LLMJudge evaluators; omit to skip them')
    parser.add_argument('--mode', choices=[*MODES, 'all'], default='single')
    parser.add_argument('--repeat', type=int, default=1, help='attempts per case, for variance')
    parser.add_argument('--max-turns', type=int, default=DEFAULT_MAX_TURNS)
    parser.add_argument(
        '--concurrency',
        type=int,
        default=1,
        help='cases run at once; stateful tools and mounts are shared, so repeats of one task must not overlap',
    )
    parser.add_argument(
        '--dry-run',
        action='store_true',
        help="execute each task's reference solution instead of calling a model",
    )
    parser.add_argument('--reports', type=Path, default=REPORTS_DIR)
    args = parser.parse_args(argv)
    args.prompt = [variant for variant in args.prompt.split(',') if variant]

    reports = asyncio.run(_run_all(args))
    write_reports(reports, args.reports)

    failed = [
        case.name
        for report in reports
        for case in report.cases
        if not all(assertion.value for assertion in case.assertions.values())
    ] + [failure.name for report in reports for failure in report.failures]
    print(f'\n{len(failed)} failing case(s). Reports written to {args.reports}')
    if failed and args.dry_run:
        print('A failing reference solution means the task is wrong or Monty has a real gap:')
        for name in failed:
            print(f'  {name}')
    return 1 if failed else 0


if __name__ == '__main__':
    raise SystemExit(main())
