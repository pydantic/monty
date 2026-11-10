import argparse
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('runs', type=Path, nargs='+')
p.add_argument('--flying-circus-source', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--candidate', default='threaded')
p.add_argument('--baseline', default='baseline')
a = p.parse_args()
# Use the explicitly selected Flying Circus checkout for both adapters and statistics.
sys.path.insert(0, str(a.flying_circus_source.resolve()))
from flying_circus.differences import difference  # noqa: E402

groups = defaultdict(lambda: defaultdict(list))
hashes = defaultdict(set)
rejected = 0
incomplete = {}
for run in a.runs:
    metadata = json.loads((run / 'metadata.json').read_text())
    if metadata.get('exploratory_allow_contention'):
        raise ValueError(f'Exploratory samples cannot enter the final summary: {run}')
    for label, binary in metadata['binaries'].items():
        hashes[label].add(binary['sha256'])
    local = defaultdict(lambda: defaultdict(list))
    for line in (run / 'samples.jsonl').read_text().splitlines():
        row = json.loads(line)
        if not row['accepted']:
            rejected += 1
        elif row['sample'] >= 0:
            local[row['workload']][row['runtime']].append(row['ns'] / 1e6)
    command = metadata['command']
    expected = metadata.get('samples', int(command[command.index('--samples') + 1]) if '--samples' in command else 30)
    required = [a.baseline, a.candidate] + (['cpython'] if 'cpython' in metadata['binaries'] else [])
    missing = []
    for workload in metadata['sources']:
        runtimes = local[workload]
        if any(len(runtimes[label]) != expected for label in required):
            missing.append(workload)
            continue
        for label, values in runtimes.items():
            groups[workload][label].extend(values)
    if missing:
        incomplete[str(run)] = missing
for label, values in hashes.items():
    if len(values) != 1:
        raise ValueError(f'Mixed executable hashes for {label}: {values}')
results = {}
for workload, runtimes in groups.items():
    if not runtimes.get(a.baseline) or not runtimes.get(a.candidate):
        continue
    comparison = difference(
        runtimes[a.baseline], runtimes[a.candidate], threshold=2, absolute=0.01, comparisons=2 * len(groups)
    )
    results[workload] = {
        'median_ms': {label: statistics.median(values) for label, values in runtimes.items()},
        'sample_count': {label: len(values) for label, values in runtimes.items()},
        'comparison': comparison,
    }
    if runtimes.get('cpython'):
        results[workload]['cpython_comparison'] = difference(
            runtimes['cpython'], runtimes[a.candidate], threshold=2, absolute=0.01, comparisons=2 * len(groups)
        )
a.output.mkdir(parents=True, exist_ok=True)
summary = {
    'runs': list(map(str, a.runs)),
    'binary_sha256': {k: next(iter(v)) for k, v in hashes.items()},
    'rejected_samples': rejected,
    'threshold_percent': 2,
    'absolute_threshold_ms': 0.01,
    'incomplete_runs': incomplete,
    'results': results,
}
(a.output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
table = [
    '| Workload | Baseline ms | Threaded ms | CPython ms | Change | Status |',
    '| --- | ---: | ---: | ---: | ---: | --- |',
]
counts = defaultdict(int)
for name, result in results.items():
    medians, comparison = result['median_ms'], result['comparison']
    counts[comparison['status']] += 1
    table.append(
        f'| {name} | {medians[a.baseline]:.3f} | {medians[a.candidate]:.3f} | '
        f'{medians.get("cpython", float("nan")):.3f} | {comparison["change_percent"]:+.1f}% | {comparison["status"]} |'
    )
(a.output / 'table.md').write_text('\n'.join(table) + '\n')
print(dict(counts))
if incomplete:
    print('Excluded incomplete workloads:', incomplete)
for name, result in results.items():
    if result['comparison']['status'] in ('regression', 'inconclusive'):
        print(name, result['comparison'])
