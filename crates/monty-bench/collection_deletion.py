# /// script
# requires-python = ">=3.14"
# dependencies = ["pydantic-monty==1.1.0"]
# ///
"""Compare release workers on collection deletion and unchanged-operation controls.

Build each revision with `cargo build -p monty-runtime --release --locked`, copy
the binaries, then run this script with --baseline PATH --candidate PATH.
The timed region is a complete feed on a warm worker: compilation, input
transport, execution, and output conversion. Checkout and worker startup are
excluded. Both workers execute identical code, and every output is compared.
"""

import argparse
import hashlib
import json
import platform
import statistics
import time
from pathlib import Path

from pydantic_monty import Monty, MontyRuntimeError, ResourceLimits

COUNTER = """
from collections import Counter
counts = Counter({i: i % 2 for i in range(n)})
positive = +counts
[len(positive), sum(positive.values()), list(positive)]
"""
SET_REMOVE = """
pending = set(range(n))
for i in range(0, n, 2):
    pending.remove(i)
[len(pending), sum(pending)]
"""
INVENTORY = """
import json
rows = json.loads(payload)
records = {row['id']: row for row in rows}
for row in rows:
    if row['quantity'] == 0:
        records.pop(row['id'])
[len(records), sum(row['price'] for row in records.values()), list(records)]
"""
CHURN = """
cache = {i: i * 3 for i in range(window)}
checksum = 0
for i in range(window, window + events):
    checksum += cache.pop(i - window)
    cache[i] = i * 3
    checksum += cache[i - window // 2]
[len(cache), checksum, sum(cache.values())]
"""
DENSE_DICT = """
d = {i: i * 3 for i in range(n)}
checksum = 0
for _ in range(20):
    for i in range(n):
        checksum += d[i]
    checksum += sum(d.values())
[len(d), checksum]
"""
DENSE_SET = """
s = set(range(n))
checksum = 0
for _ in range(20):
    for i in range(n):
        checksum += i in s
    checksum += sum(s)
[len(s), checksum]
"""
DENSE_FROZENSET = """
s = frozenset(range(n))
checksum = 0
for _ in range(20):
    for i in range(n):
        checksum += i in s
    checksum += sum(s)
[len(s), checksum]
"""
SMALL_DICTS = """
checksum = 0
for i in range(n):
    d = {'id': i, 'quantity': i % 3, 'price': 7}
    checksum += d['id'] + d['quantity'] + sum(d.values())
checksum
"""
SMALL_FROZENSETS = """
checksum = 0
for i in range(n):
    values = frozenset((i, i + 1, i + 2))
    checksum += len(values) + sum(values)
checksum
"""
SPARSE_READ = """
d = {i: i * 3 for i in range(n)}
for i in range(0, n, 3):
    d.pop(i)
checksum = 0
for _ in range(50):
    checksum += sum(d) + sum(d.values())
[len(d), checksum]
"""


def workloads():
    for n in [5_000, 10_000, 20_000, 40_000]:
        yield f'counter/{n}', COUNTER, {'n': n}
        yield f'set_remove/{n}', SET_REMOVE, {'n': n}
    for n in [10_000, 20_000]:
        payload = json.dumps(
            [
                {
                    'id': f'sku-{i:08d}',
                    'quantity': i % 2,
                    'price': i % 199 + 1,
                    'description': 'Inventory record',
                }
                for i in range(n)
            ]
        )
        yield f'inventory/{n}', INVENTORY, {'payload': payload}
    for window in [1_000, 10_000]:
        yield f'churn/{window}', CHURN, {'window': window, 'events': 20_000}
    for name, code in [
        ('dense_dict', DENSE_DICT),
        ('dense_set', DENSE_SET),
        ('dense_frozenset', DENSE_FROZENSET),
        ('small_dicts', SMALL_DICTS),
        ('small_frozensets', SMALL_FROZENSETS),
        ('sparse_read', SPARSE_READ),
    ]:
        yield name, code, {'n': 10_000}


SNAPSHOT_SETUP = """
d = {i: [i] for i in range(128)}
s = set(range(128))
for i in range(63):
    d.pop(i)
    s.remove(i)
keys, values, items, members = iter(d), iter(d.values()), iter(d.items()), iter(s)
next(keys)
next(values)
next(items)
first_member = next(members)
d.pop(127)
d[128] = [128]
s.remove(127)
s.add(128)
frozen = frozenset(range(128))
hash(frozen)
frozen_members = iter(frozen)
next(frozen_members)
None
"""
SNAPSHOT_CHECK = """
[list(keys), list(values), list(items), sorted([first_member] + list(members)),
 list(d), len(s), frozen, list(frozen_members), hash(frozen)]
"""
SET_CHURN = """
s = set(range(window))
for i in range(window, window + events):
    s.remove(i - window)
    s.add(i)
[len(s), sum(s)]
"""


def check_contracts(pools):
    """Cross-load live iterators and bracket passing memory limits to 16 KiB.

    A passing configured limit includes allocator headroom and preflight checks;
    it is not a measurement of peak RSS or exact live allocation size.
    """
    snapshots = []
    memory = []
    for label, pool in pools:
        with pool.checkout() as session:
            session.feed_run(SNAPSHOT_SETUP)
            data = session.dump()
            expected = session.feed_run(SNAPSHOT_CHECK)
            snapshots.append((label, data, expected))
        for name, code in [('dict_churn', CHURN), ('set_churn', SET_CHURN)]:
            low, high = 1024, 16 * 1024 * 1024
            trials = []

            def trial(limit):
                try:
                    with pool.checkout(limits=ResourceLimits(max_memory=limit)) as session:
                        session.feed_run(code, inputs={'window': 10_000, 'events': 20_000})
                    passed = True
                except MontyRuntimeError as exc:
                    if not isinstance(exc.exception(), MemoryError):
                        raise
                    passed = False
                trials.append({'limit': limit, 'passed': passed})
                return passed

            if trial(low):
                raise RuntimeError(f'{label}/{name}: expected a failing lower bound at {low} bytes')
            if not trial(high):
                raise RuntimeError(f'{label}/{name}: no passing upper bound within {high} bytes')
            while high - low > 16 * 1024:
                limit = (low + high) // 2
                if trial(limit):
                    high = limit
                else:
                    low = limit
            memory.append(
                {'binary': label, 'workload': name, 'failing_limit': low, 'passing_limit': high, 'trials': trials}
            )
    loads = []
    for label, pool in pools:
        for source, data, expected in snapshots:
            with pool.checkout() as session:
                session.load_session(data)
                assert session.feed_run(SNAPSHOT_CHECK) == expected, (source, label)
            loads.append({'source': source, 'target': label, 'bytes': len(data), 'equal': True})
    return {'snapshots': loads, 'memory_limits': memory}


def measure(pool, code, inputs):
    with pool.checkout() as session:
        start = time.perf_counter_ns()
        result = session.feed_run(code, inputs=inputs)
        return (time.perf_counter_ns() - start) / 1e6, result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('collection-deletion.json'))
    parser.add_argument('--repeats', type=int, default=7)
    parser.add_argument('--contracts', action='store_true', help='Check memory limits and snapshot compatibility')
    args = parser.parse_args()
    if args.repeats <= 0:
        parser.error('--repeats must be positive')
    report = {
        'platform': platform.platform(),
        'python': platform.python_version(),
        'binary_sha256': {
            label: hashlib.sha256(path.read_bytes()).hexdigest()
            for label, path in [('baseline', args.baseline), ('candidate', args.candidate)]
        },
        'warmups': 2,
        'repeats': args.repeats,
        'results': [],
    }
    with (
        Monty(binary_path=args.baseline, min_processes=1, max_processes=1, request_timeout=60) as baseline,
        Monty(binary_path=args.candidate, min_processes=1, max_processes=1, request_timeout=60) as candidate,
    ):
        pools = [('baseline', baseline), ('candidate', candidate)]
        if args.contracts:
            report.update(check_contracts(pools))
            args.output.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(report, indent=2))
            return
        for name, code, inputs in workloads():
            samples = {label: [] for label, _ in pools}
            expected = None
            for rep in range(args.repeats + 2):
                for label, pool in pools if rep % 2 == 0 else pools[::-1]:
                    elapsed, result = measure(pool, code, inputs)
                    if expected is None:
                        expected = result
                    assert result == expected, (name, label)
                    if rep >= 2:
                        samples[label].append(elapsed)
            row = {
                'workload': name,
                'samples_ms': samples,
                'median_ms': {label: statistics.median(values) for label, values in samples.items()},
            }
            report['results'].append(row)
            args.output.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(row), flush=True)


if __name__ == '__main__':
    main()
