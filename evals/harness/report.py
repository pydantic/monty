"""Cross-experiment reports built from pydantic-evals `EvaluationReport`s.

pydantic-evals prints one table per experiment. `scoreboard.md` puts every experiment
(prompt × model × mode) side by side, per objective axis, and `feature_gaps.md` ranks
the Monty gaps recorded in the cases' `gaps` attribute. `scoreboard.json` holds the
per-case rows both are built from.
"""

from __future__ import annotations

import json
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from statistics import mean
from typing import Any

from pydantic_evals.reporting import EvaluationReport, ReportCase

from .metrics import ATTR, METRIC
from .task import Task

__all__ = ('write_reports',)

_VARIANT_ORDER = ('v0_bare', 'v1_current', 'v2_accurate', 'v3_idioms', 'v4_codemode', 'v5_minimal')


def write_reports(reports: list[EvaluationReport[Task, Any, Any]], directory: Path) -> None:
    """Write `scoreboard.md`, `scoreboard.json` and `feature_gaps.md` into `directory`."""
    rows = [_Row.from_case(report, case) for report in reports for case in report.cases]
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'scoreboard.json').write_text(json.dumps([row.as_json() for row in rows], indent=2) + '\n')
    (directory / 'scoreboard.md').write_text(_scoreboard(rows, reports))
    (directory / 'feature_gaps.md').write_text(_feature_gaps(rows))


@dataclass(frozen=True)
class _Row:
    """One case of one experiment, flattened for aggregation."""

    experiment: str
    prompt: str
    model: str
    mode: str
    case: ReportCase[Task, Any, Any]

    @classmethod
    def from_case(cls, report: EvaluationReport[Task, Any, Any], case: ReportCase[Task, Any, Any]) -> _Row:
        meta = report.experiment_metadata or {}
        return cls(
            report.name, str(meta.get('prompt', '?')), str(meta.get('model', '?')), str(meta.get('mode', '?')), case
        )

    @property
    def task(self) -> str:
        return self.case.source_case_name or self.case.name

    @property
    def success(self) -> bool:
        return all(a.value for a in self.case.assertions.values())

    @property
    def gaps(self) -> list[dict[str, Any]]:
        return list(self.case.attributes.get(ATTR.GAPS) or [])

    def metric(self, name: str) -> float:
        return float(self.case.metrics.get(name, 0))

    def assertion(self, name: str) -> bool | None:
        result = self.case.assertions.get(name)
        return None if result is None else result.value

    def as_json(self) -> dict[str, Any]:
        return {
            'experiment': self.experiment,
            'prompt': self.prompt,
            'model': self.model,
            'mode': self.mode,
            'task': self.task,
            'case': self.case.name,
            'success': self.success,
            'metrics': dict(self.case.metrics),
            'attributes': dict(self.case.attributes),
            'assertions': {name: a.value for name, a in self.case.assertions.items()},
            'reasons': {name: a.reason for name, a in self.case.assertions.items() if a.reason},
            'scores': {name: s.value for name, s in self.case.scores.items()},
            'task_duration': self.case.task_duration,
        }


def _scoreboard(rows: list[_Row], reports: list[EvaluationReport[Task, Any, Any]]) -> str:
    """Render the per-axis prompt × model × mode table."""
    by_experiment: dict[str, list[_Row]] = defaultdict(list)
    for row in rows:
        by_experiment[row.experiment].append(row)

    lines = [
        '# Prompt scoreboard',
        '',
        f'{len(rows)} cases across {len({r.task for r in rows})} tasks.',
        '',
        '| Prompt | Model | Mode | Correct | 1st-try runs | Turns | Tokens | Round trips | Within budget | Lines | Nesting | Result bytes |',
        '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |',
    ]
    for name in sorted(by_experiment, key=lambda n: (_variant_rank(by_experiment[n][0].prompt), n)):
        cell = by_experiment[name]
        first = cell[0]
        budget = [r for r in cell if r.assertion('within_call_budget') is not None]
        within = (
            'n/a' if not budget else f'{mean(1.0 if r.assertion("within_call_budget") else 0.0 for r in budget):.0%}'
        )
        lines.append(
            f'| {first.prompt} | {first.model} | {first.mode} '
            f'| {mean(1.0 if r.success else 0.0 for r in cell):.0%} '
            f'| {mean(1.0 if r.assertion(ATTR.FIRST_ATTEMPT_RUNS) else 0.0 for r in cell):.0%} '
            f'| {mean(r.metric(METRIC.TURNS) for r in cell):.1f} '
            f'| {mean(r.metric(METRIC.PROMPT_TOKENS) + r.metric(METRIC.COMPLETION_TOKENS) for r in cell):,.0f} '
            f'| {mean(r.metric(METRIC.CALL_BATCHES) for r in cell):.1f} | {within} '
            f'| {mean(r.metric(METRIC.CODE_LINES) for r in cell):.0f} '
            f'| {mean(r.metric(METRIC.MAX_NESTING) for r in cell):.1f} '
            f'| {mean(r.metric(METRIC.RESULT_BYTES) for r in cell):,.0f} |'
        )

    lines += ['', '## Failures by task', '']
    failures: dict[str, list[str]] = defaultdict(list)
    for row in rows:
        for name, assertion in row.case.assertions.items():
            if not assertion.value:
                failures[row.task].append(f'{row.experiment}: {name} ({assertion.reason or "no reason"})')
    for report in reports:
        for failure in report.failures:
            failures[failure.source_case_name or failure.name].append(
                f'{report.name}: crashed: {failure.error_message}'
            )
    if not failures:
        lines.append('None.')
    for task in sorted(failures):
        lines.append(f'- **{task}**')
        lines += [f'    - {detail}' for detail in failures[task]]
    return '\n'.join(lines) + '\n'


def _feature_gaps(rows: list[_Row]) -> str:
    """Rank the Monty gaps that agent-written code hit, by how many tasks each blocked."""
    by_symbol: dict[tuple[str, str], list[tuple[_Row, dict[str, Any]]]] = defaultdict(list)
    for row in rows:
        for gap in row.gaps:
            by_symbol[(str(gap['kind']), str(gap['symbol']))].append((row, gap))

    lines = [
        '# Feature gaps hit by agent-written code',
        '',
        'Ranked by how many distinct tasks the gap blocked. A gap only the weaker prompts hit',
        'is a prompt problem; a gap the strongest prompt still hits is a feature to build.',
        '',
    ]
    if not by_symbol:
        return '\n'.join(lines + ['No feature gaps recorded.']) + '\n'

    lines += [
        '| Kind | Symbol | Tasks | Hits | Models | Best prompt still failing | Certain | Documented in |',
        '| --- | --- | ---: | ---: | --- | --- | :---: | --- |',
    ]

    def rank(item: tuple[tuple[str, str], list[tuple[_Row, dict[str, Any]]]]) -> tuple[int, int]:
        _, entries = item
        return (-len({r.task for r, _ in entries}), -len(entries))

    for (kind, symbol), entries in sorted(by_symbol.items(), key=rank):
        tasks = sorted({r.task for r, _ in entries})
        models = sorted({r.model for r, _ in entries})
        best = max({r.prompt for r, _ in entries}, key=_variant_rank)
        certain = all(bool(g['certain']) for _, g in entries)
        doc = next((str(g['doc']) for _, g in entries if g['doc']), '-')
        lines.append(
            f'| {kind} | `{symbol}` | {len(tasks)} | {len(entries)} | {", ".join(models)} '
            f'| {best} | {"yes" if certain else "no"} | {doc} |'
        )

    lines += ['', '## Where each gap was hit', '']
    for (kind, symbol), entries in sorted(by_symbol.items(), key=rank):
        lines += [f'### `{symbol}` ({kind})', '']
        seen: set[str] = set()
        for row, gap in entries:
            source = str(gap.get('source_line') or '')
            key = f'{row.task}|{source}'
            if key in seen:
                continue
            seen.add(key)
            lines.append(f'- `{row.task}` ({row.prompt}): `{source or "?"}`')
            lines.append(f'    - {gap["message"]}')
        lines.append('')
    return '\n'.join(lines) + '\n'


def _variant_rank(variant: str) -> int:
    """Order prompt variants weakest to strongest; unknown (generated) names sort strongest."""
    try:
        return _VARIANT_ORDER.index(variant)
    except ValueError:
        return len(_VARIANT_ORDER)
