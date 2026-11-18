# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "scipy"]
# ///
"""Fit the work clock's weights to measured time, and regenerate the weight tables.

`make fit-work-clock` runs this after collecting, for every calibration script, how often each
opcode ran and each work event was charged (`counts.json`, from `monty-bench`) and how long the
script took in the wasm build (`wasm.json`, from `calibrate-work-clock.mjs`).

Every weight is solved for at once: the time a script took should equal the sum over its opcodes
and events of count times weight. The fit minimises each script's relative error, so a short
script counts as much as a long one, and pulls each weight towards its current value, so a weight
the data says little about stays put. Weights are in thousandths of a unit, where a unit is
`NANOS_PER_UNIT` nanoseconds of the wasm build.

Without `--write` it only reports what the fit would change.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
from scipy.optimize import lsq_linear

ROOT = Path(__file__).parent.parent
EVENT_WEIGHTS = ROOT / 'crates/monty-types/src/resource/work_weights.rs'
OPCODE_WEIGHTS = ROOT / 'crates/monty/src/bytecode/op_weights.rs'

# How strongly a weight is held to its current value, against one script's relative error.
PRIOR_STRENGTH = 0.3
# The most one fit may scale a weight by, either way. Opcodes that always run together share
# their cost arbitrarily; this keeps either from being fitted to nothing.
MAX_CHANGE = 4.0
# Thousandths of a unit below which no opcode is fitted: fetching and dispatching one costs
# this much, and without it repeated fits walk the commonest opcodes down to nothing.
OPCODE_FLOOR = 500
# Rounds of reweighting towards the scripts the fit serves worst, to shrink the largest error.
REWEIGHT_ROUNDS = 12
# An opcode whose fitted weight is within this share of the base is charged the base alone,
# which keeps the hottest handlers free of an extra add.
BASE_TOLERANCE = 0.1
# The share of all instructions an opcode needs before it can set the base weight.
BASE_MIN_SHARE = 0.01
# How far the two builds' charges for one script may differ before its counts are not trusted.
BUILD_DISAGREEMENT = 0.1
# Scripts further than this from their measured time are listed.
TARGET_ERROR = 0.5


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('data', type=Path, help='directory holding counts.json and wasm.json')
    parser.add_argument('--write', action='store_true', help='regenerate the weight tables')
    args = parser.parse_args()

    data = Data.load(args.data)
    print(f'{len(data.names)} scripts, {len(data.features)} weights in use')
    report('current weights', data, data.prior)

    fitted = fit(data)
    report('fitted weights', data, fitted)

    tables = Tables.from_weights(data, fitted)
    report('fitted weights as written', data, tables.weights(data))
    tables.print_changes(data)
    if args.write:
        tables.write(data)
        print(f'wrote {EVENT_WEIGHTS.relative_to(ROOT)} and {OPCODE_WEIGHTS.relative_to(ROOT)}')


class Data:
    """The counts and times of every script both builds measured."""

    def __init__(self, counts: dict, wasm: dict) -> None:
        self.base: int = next(e['milli_units'] for e in counts['events'] if e['name'] == 'Instruction')
        self.opcodes: list[dict] = counts['opcodes']
        self.events: list[dict] = counts['events']
        # A script is left out if the wasm build could not time it (it trapped, or charged
        # nothing) or charged it differently: its counts would then describe other work.
        times = wasm['times']
        rows = [
            row
            for row in counts['rows']
            if (time := times.get(row['name']))
            and time['wallMicros']
            and abs(time['units'] * 1000 / row['milli_units'] - 1) <= BUILD_DISAGREEMENT
        ]
        print(f'{len(counts["rows"]) - len(rows)} of {len(counts["rows"])} scripts left out')
        self.names: list[str] = [row['name'] for row in rows]

        opcode_prior = {op['name']: self.base + op['extra'] for op in self.opcodes}
        event_prior = {e['name']: e['milli_units'] for e in self.events if e['name'] != 'Instruction'}
        used_opcodes = sorted({name for row in rows for name in row['opcodes']})
        used_events = sorted({name for row in rows for name in row['events']})
        # A feature is `('opcode', name)` or `('event', name)`; its weight is thousandths of a unit.
        self.features: list[tuple[str, str]] = [('opcode', n) for n in used_opcodes] + [
            ('event', n) for n in used_events
        ]
        self.prior = np.array(
            [opcode_prior[n] if kind == 'opcode' else event_prior[n] for kind, n in self.features], float
        )
        self.counts = np.array(
            [[row['opcodes' if kind == 'opcode' else 'events'].get(n, 0) for kind, n in self.features] for row in rows],
            float,
        )
        # What each script's wasm time is worth in thousandths of a unit.
        nanos_per_unit = wasm['nanosPerUnit']
        self.worth = np.array([wasm['times'][name]['wallMicros'] * 1e6 / nanos_per_unit for name in self.names])

    @classmethod
    def load(cls, directory: Path) -> Data:
        return cls(
            json.loads((directory / 'counts.json').read_text()), json.loads((directory / 'wasm.json').read_text())
        )

    def ratios(self, weights: np.ndarray) -> np.ndarray:
        """Each script's measured time over the time `weights` would report."""
        return self.worth / (self.counts @ weights)


def fit(data: Data) -> np.ndarray:
    """Solves for non-negative weights minimising relative error, held towards the current ones."""
    # Solved in multiples of the current weights, which keeps the system well conditioned.
    share = data.counts * data.prior / data.worth[:, None]
    prior_rows = np.sqrt(PRIOR_STRENGTH) * np.eye(len(data.prior))
    prior_target = np.full(len(data.prior), np.sqrt(PRIOR_STRENGTH))
    # No instruction is dispatched for less than the floor, whatever its neighbours absorb.
    floor = np.array([OPCODE_FLOOR if kind == 'opcode' else 0 for kind, _ in data.features]) / data.prior
    lower = np.maximum(1 / MAX_CHANGE, floor)
    upper = np.maximum(MAX_CHANGE, lower * 1.001)
    emphasis = np.ones(len(data.names))
    weights = data.prior
    for _ in range(REWEIGHT_ROUNDS):
        matrix = np.vstack([share * emphasis[:, None], prior_rows])
        target = np.concatenate([emphasis, prior_target])
        weights = lsq_linear(matrix, target, bounds=(lower, upper)).x * data.prior
        # Lean on the scripts furthest out, so the fit trades average error for worst case.
        error = np.abs(np.log(data.ratios(weights)))
        emphasis = np.clip(emphasis * (1 + error), 1, 20)
    return weights


def report(title: str, data: Data, weights: np.ndarray) -> None:
    """Prints how far each group of scripts sits from its measured time under `weights`."""
    ratios = data.ratios(weights)
    print(f'\n{title}: measured time / reported time')
    print(f'  {"group":<10} {"scripts":>7} {"p5":>6} {"p50":>6} {"p95":>6} {"min":>6} {"max":>6} {"within 50%":>11}')
    groups = ['all', *sorted({name.split('__')[0] for name in data.names})]
    for group in groups:
        chosen = np.array([group == 'all' or name.startswith(group + '__') for name in data.names])
        values = ratios[chosen]
        within = np.mean((values >= 1 / (1 + TARGET_ERROR)) & (values <= 1 + TARGET_ERROR))
        p5, p50, p95 = np.percentile(values, [5, 50, 95])
        print(
            f'  {group:<10} {len(values):>7} {p5:>6.2f} {p50:>6.2f} {p95:>6.2f} '
            f'{values.min():>6.2f} {values.max():>6.2f} {within:>10.0%}'
        )
    outside = [(ratio, name) for ratio, name in zip(ratios, data.names) if not 1 / 1.5 <= ratio <= 1.5]
    for ratio, name in sorted(outside)[:12] + sorted(outside)[-12:] if len(outside) > 24 else sorted(outside):
        print(f'    {ratio:>6.2f}  {name}')


class Tables:
    """The fitted weights as the generated tables hold them: a base, opcode extras and event weights."""

    def __init__(self, base: int, extras: dict[str, int], events: dict[str, int]) -> None:
        self.base = base
        self.extras = extras
        self.events = events

    @classmethod
    def from_weights(cls, data: Data, weights: np.ndarray) -> Tables:
        fitted = {feature: weight for feature, weight in zip(data.features, weights)}
        totals = data.counts.sum(axis=0)
        instructions = sum(total for (kind, _), total in zip(data.features, totals) if kind == 'opcode')
        common = [
            fitted[feature]
            for feature, total in zip(data.features, totals)
            if feature[0] == 'opcode' and total >= BASE_MIN_SHARE * instructions
        ]
        base = max(1, round(min(common)))

        # An opcode the data never ran keeps the total it had, or the base if that is now higher.
        extras = {op['name']: max(0, data.base + op['extra'] - base) for op in data.opcodes}
        for (kind, name), weight in fitted.items():
            if kind == 'opcode':
                extra = weight - base
                extras[name] = round(extra) if extra > BASE_TOLERANCE * base else 0

        events = {e['name']: e['milli_units'] for e in data.events}
        events['Instruction'] = base
        for (kind, name), weight in fitted.items():
            if kind == 'event':
                events[name] = max(1, round(weight)) if name == 'AllocatedByte' else round(weight)
        # An allocation is charged as whole bytes of overhead.
        events['Allocation'] -= events['Allocation'] % events['AllocatedByte']
        return cls(base, extras, events)

    def weights(self, data: Data) -> np.ndarray:
        return np.array(
            [self.base + self.extras[n] if kind == 'opcode' else self.events[n] for kind, n in data.features], float
        )

    def print_changes(self, data: Data) -> None:
        """Lists every weight the tables change, largest count first."""
        totals = dict(zip(data.features, data.counts.sum(axis=0)))
        print(f'\nbase instruction weight: {data.base} -> {self.base}')
        print(f'  {"weight":<28} {"count":>12} {"now":>8} {"fitted":>8}')
        for kind, name in sorted(data.features, key=lambda feature: -totals[feature]):
            if kind == 'opcode':
                now = data.base + next(op['extra'] for op in data.opcodes if op['name'] == name)
                new = self.base + self.extras[name]
            else:
                now = next(e['milli_units'] for e in data.events if e['name'] == name)
                new = self.events[name]
            if now != new:
                print(f'  {kind + " " + name:<28} {totals[kind, name]:>12.0f} {now:>8} {new:>8}')

    def write(self, data: Data) -> None:
        events = '\n'.join(f'    {self.events[e["name"]]:_}, // {e["name"]}' for e in data.events)
        EVENT_WEIGHTS.write_text(
            '//! Generated by `make fit-work-clock`; do not edit.\n\n'
            'use strum::EnumCount;\n\n'
            'use super::work_event::WorkEvent;\n\n'
            '/// Thousandths of a unit per occurrence of each [`WorkEvent`], in declaration order.\n'
            'pub(super) const EVENT_MILLI_UNITS: [u32; WorkEvent::COUNT] = [\n'
            f'{events}\n];\n'
        )
        names = {op['byte']: op['name'] for op in data.opcodes}
        opcodes = '\n'.join(
            f'    {self.extras[names[byte]] if byte in names else 0:_}, // {byte}: {names.get(byte, "unused")}'
            for byte in range(256)
        )
        OPCODE_WEIGHTS.write_text(
            '//! Generated by `make fit-work-clock`; do not edit.\n\n'
            "/// Thousandths of a unit each opcode is charged beyond an instruction's own, by opcode byte.\n"
            'pub(super) const OPCODE_EXTRA_MILLI_UNITS: [u32; 256] = [\n'
            f'{opcodes}\n];\n'
        )


if __name__ == '__main__':
    main()
