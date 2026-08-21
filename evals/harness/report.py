"""Aggregates attempts into the two reports the suite exists to produce.

`scoreboard.md` answers "which prompt should we ship", per objective axis rather than
as one blended number, because the axes conflict and the right trade-off depends on
the caller.

`feature_gaps.md` answers "what should we build next". Its load-bearing column is the
best prompt variant that still hits a gap: a gap only the weak prompts hit is a
documentation problem, while a gap the best prompt still hits is a feature.
"""

from __future__ import annotations

import json
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from statistics import mean

from .metrics import AttemptMetrics

__all__ = ('write_reports',)

_VARIANT_ORDER = ('v0_bare', 'v1_current', 'v2_accurate', 'v3_idioms', 'v4_codemode', 'v5_minimal')


def write_reports(results: list[AttemptMetrics], directory: Path) -> None:
    """Write `scoreboard.md`, `scoreboard.json` and `feature_gaps.md` into `directory`."""
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'scoreboard.json').write_text(json.dumps([r.as_row() for r in results], indent=2) + '\n')
    (directory / 'scoreboard.md').write_text(_scoreboard(results))
    (directory / 'feature_gaps.md').write_text(_feature_gaps(results))


@dataclass
class _Cell:
    """Aggregated attempts for one (prompt, model, mode) combination."""

    attempts: list[AttemptMetrics]

    @property
    def success_rate(self) -> float:
        return mean(1.0 if a.success else 0.0 for a in self.attempts)

    @property
    def first_attempt_rate(self) -> float:
        return mean(1.0 if a.first_attempt_runs else 0.0 for a in self.attempts)

    @property
    def mean_turns(self) -> float:
        return mean(a.turns_used for a in self.attempts)

    @property
    def mean_tokens(self) -> float:
        return mean(a.total_tokens for a in self.attempts)

    @property
    def mean_batches(self) -> float:
        return mean(a.call_batches for a in self.attempts)

    @property
    def batch_efficiency(self) -> float | None:
        """Fraction of attempts that made no more round trips than the task allows.

        The time axis in one number. `None` when no task in the cell pins an expectation.
        """
        scored = [a for a in self.attempts if a.expected_call_batches is not None]
        if not scored:
            return None
        return mean(1.0 if a.call_batches <= (a.expected_call_batches or 0) else 0.0 for a in scored)

    @property
    def mean_lines(self) -> float:
        return mean(a.code_lines for a in self.attempts)

    @property
    def mean_nesting(self) -> float:
        return mean(a.max_nesting for a in self.attempts)

    @property
    def mean_result_bytes(self) -> float:
        return mean(a.result_bytes for a in self.attempts)


def _scoreboard(results: list[AttemptMetrics]) -> str:
    """Render the per-axis prompt × model × mode table."""
    cells: dict[tuple[str, str, str], list[AttemptMetrics]] = defaultdict(list)
    for attempt in results:
        cells[(attempt.prompt_variant, attempt.model, attempt.mode)].append(attempt)

    lines = [
        '# Prompt scoreboard',
        '',
        f'{len(results)} attempts across {len({r.task for r in results})} tasks.',
        '',
        'Axes are reported separately on purpose — they conflict, so the weighting is the',
        "reader's call. Correctness gates the rest: a cheap prompt that gets the wrong",
        'answer is not a cheap prompt.',
        '',
        '| Prompt | Model | Mode | Correct | 1st-try runs | Turns | Tokens | Round trips | Within budget | Lines | Nesting | Result bytes |',
        '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |',
    ]

    for key in sorted(cells, key=lambda k: (_variant_rank(k[0]), k[1], k[2])):
        variant, model, mode = key
        cell = _Cell(cells[key])
        efficiency = cell.batch_efficiency
        lines.append(
            f'| {variant} | {model} | {mode} '
            f'| {cell.success_rate:.0%} | {cell.first_attempt_rate:.0%} | {cell.mean_turns:.1f} '
            f'| {cell.mean_tokens:,.0f} | {cell.mean_batches:.1f} '
            f'| {"n/a" if efficiency is None else f"{efficiency:.0%}"} '
            f'| {cell.mean_lines:.0f} | {cell.mean_nesting:.1f} | {cell.mean_result_bytes:,.0f} |'
        )

    lines += ['', '## Failures by task', '']
    failures: dict[str, list[str]] = defaultdict(list)
    for attempt in results:
        if not attempt.success:
            failures[attempt.task].append(f'{attempt.prompt_variant}/{attempt.mode}: {attempt.detail}')
    if not failures:
        lines.append('None.')
    else:
        for task in sorted(failures):
            lines.append(f'- **{task}**')
            lines += [f'  - {detail}' for detail in failures[task]]
    return '\n'.join(lines) + '\n'


def _feature_gaps(results: list[AttemptMetrics]) -> str:
    """Rank the Monty gaps that agent-written code actually hit."""
    by_symbol: dict[tuple[str, str], list[tuple[AttemptMetrics, dict[str, object]]]] = defaultdict(list)
    for attempt in results:
        for gap in attempt.gaps:
            by_symbol[(str(gap['kind']), str(gap['symbol']))].append((attempt, gap))

    lines = [
        '# Feature gaps hit by agent-written code',
        '',
        'Ranked by how many distinct tasks the gap blocked. **Best prompt still failing**',
        'is the decision column: a gap only the weaker prompts hit is a prompt or docs',
        'problem; a gap the strongest prompt still hits is a feature to build.',
        '',
    ]
    if not by_symbol:
        return '\n'.join(lines + ['No feature gaps recorded.']) + '\n'

    lines += [
        '| Kind | Symbol | Tasks | Hits | Models | Best prompt still failing | Certain | Documented in |',
        '| --- | --- | ---: | ---: | --- | --- | :---: | --- |',
    ]

    def rank(item: tuple[tuple[str, str], list[tuple[AttemptMetrics, dict[str, object]]]]) -> tuple[int, int]:
        _, entries = item
        return (-len({a.task for a, _ in entries}), -len(entries))

    for (kind, symbol), entries in sorted(by_symbol.items(), key=rank):
        tasks = sorted({a.task for a, _ in entries})
        models = sorted({a.model for a, _ in entries})
        variants = {a.prompt_variant for a, _ in entries}
        best = max(variants, key=_variant_rank)
        certain = all(bool(g['certain']) for _, g in entries)
        doc = next((str(g['doc']) for _, g in entries if g['doc']), '—')
        lines.append(
            f'| {kind} | `{symbol}` | {len(tasks)} | {len(entries)} | {", ".join(models)} '
            f'| {best} | {"yes" if certain else "no"} | {doc} |'
        )

    lines += ['', '## Where each gap was hit', '']
    for (kind, symbol), entries in sorted(by_symbol.items(), key=rank):
        lines.append(f'### `{symbol}` ({kind})')
        lines.append('')
        seen: set[str] = set()
        for attempt, gap in entries:
            source = str(gap.get('source_line') or '')
            key = f'{attempt.task}|{source}'
            if key in seen:
                continue
            seen.add(key)
            lines.append(f'- `{attempt.task}` ({attempt.prompt_variant}): `{source or "?"}`')
            lines.append(f'  - {gap["message"]}')
        lines.append('')
    return '\n'.join(lines) + '\n'


def _variant_rank(variant: str) -> int:
    """Order prompt variants weakest to strongest; unknown names sort last.

    An optimiser-generated variant is treated as the strongest, since it only exists
    because it beat the hand-written ones.
    """
    try:
        return _VARIANT_ORDER.index(variant)
    except ValueError:
        return len(_VARIANT_ORDER)
