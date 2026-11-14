"""Fit ns/op from each accepted NOP run, then report the median of the slopes."""

import argparse
import csv
import json
import statistics
from collections import defaultdict
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('run', type=Path)
args = parser.parse_args()
slopes = defaultdict(list)
for accepted in json.loads((args.run / 'accepted.json').read_text()):
    rows = defaultdict(lambda: defaultdict(list))
    with Path(accepted['path']).open() as source:
        for row in csv.DictReader(source):
            rows[row['interpreter']][int(row['nops'])].append(float(row['ns']))
    for interpreter, groups in rows.items():
        points = [(count, statistics.median(values)) for count, values in groups.items()]
        mean_x = statistics.mean(x for x, _ in points)
        mean_y = statistics.mean(y for _, y in points)
        slope = sum((x - mean_x) * (y - mean_y) for x, y in points) / sum((x - mean_x) ** 2 for x, _ in points)
        label = accepted['label'] if interpreter == 'monty' else interpreter
        slopes[label].append(slope)
result = {
    label: {'median_ns_per_op': statistics.median(values), 'round_slopes': values} for label, values in slopes.items()
}
print(json.dumps(result, indent=2))
