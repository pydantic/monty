"""Summarize accepted compile-once runs without mixing executable profiles."""

import argparse
import csv
import json
import statistics
from collections import defaultdict
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('run', type=Path)
args = parser.parse_args()
rounds = defaultdict(lambda: defaultdict(list))
for accepted in json.loads((args.run / 'accepted.json').read_text()):
    groups = defaultdict(list)
    with Path(accepted['path']).open() as source:
        for row in csv.DictReader(source):
            groups[row['case'], row['interpreter']].append(float(row['ns']) / 1e6)
    for (case, interpreter), values in groups.items():
        label = accepted['label'] if interpreter == 'monty' else interpreter
        rounds[case][label].append(statistics.median(values))
summary = {
    case: {
        label: {'median_ms': statistics.median(values), 'round_medians_ms': values}
        for label, values in runtimes.items()
    }
    for case, runtimes in rounds.items()
}
print(json.dumps(summary, indent=2))
