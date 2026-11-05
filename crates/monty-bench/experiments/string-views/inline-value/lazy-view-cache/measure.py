"""Compare optional view metadata with the published combined-cache layout."""

import argparse
import ast
import collections
import csv
import hashlib
import json
import os
import random
import statistics
import subprocess
import tempfile
from pathlib import Path


def iterations(source):
    loop = next(n for n in ast.parse(source).body if isinstance(n, ast.For))
    return loop.iter.args[0].value


def run(executable, name, directory):
    env = dict(os.environ, MONTY_RSS_CASE=name, MONTY_RSS_FIXTURES=str(directory))
    with tempfile.TemporaryFile(mode='w+') as output:
        child = subprocess.Popen(
            [str(executable), 'measure_peak_rss', '--ignored', '--test-threads=1'],
            env=env,
            stdout=output,
            stderr=subprocess.STDOUT,
        )
        _, status, usage = os.wait4(child.pid, 0)
        child.returncode = os.waitstatus_to_exitcode(status)
        if child.returncode:
            output.seek(0)
            raise RuntimeError(f'{name}: {output.read()}')
    return (usage.ru_utime + usage.ru_stime) * 1e9, usage.ru_maxrss


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('/private/tmp/monty-lazy-view-cache'))
    parser.add_argument('--fixtures', type=Path, default=Path('/private/tmp/monty-inline-benchmark/fixtures'))
    parser.add_argument('--hot', type=Path, default=Path('/private/tmp/monty-inline-benchmark/hot'))
    parser.add_argument('--agent', type=Path, default=Path('/private/tmp/monty-agent-chunking'))
    parser.add_argument('--new-exe', type=Path, default=Path('/private/tmp/monty-rss-lazy_views'))
    args = parser.parse_args()
    args.output.mkdir(exist_ok=True)
    names = [
        'unicode_owners_unindexed',
        'unicode_owners_indexed',
        'ndjson_utf8_tail',
        'prefix_remove_utf8',
        'hash_1_views',
        'hash_4_views',
        'hash_8_views',
        'hash_64_views',
        'splitlines_bulk_utf8',
    ]
    cases = [(n, args.fixtures, args.hot) for n in names]
    cases += [
        (n, args.agent / 'fixtures', args.agent / 'batch-optimized') for n in ['remainder_2048kib', 'offsets_2048kib']
    ]
    counts = {n: iterations((hot / f'{n}.py').read_text()) for n, original, hot in cases}
    builds = dict(
        arc=Path('/private/tmp/monty-rss-owned_views'),
        inline4=Path('/private/tmp/monty-rss-dual_adaptive'),
        lazy3=args.new_exe,
    )
    for build, executable in builds.items():
        for n, original, hot in cases:
            run(executable, n, original)
        print(build, 'all result checks passed', flush=True)
    cpu = []
    order = [(b, c, t) for b in builds for c in cases for t in range(5)]
    random.Random(202611056).shuffle(order)
    for i, (b, (n, original, hot), t) in enumerate(order):
        elapsed, rss = run(builds[b], n, hot)
        cpu.append([b, n, t, counts[n], elapsed, elapsed / counts[n]])
        if i % 30 == 0:
            print('CPU', i, '/', len(order), flush=True)
    with (args.output / 'cpu.csv').open('w') as f:
        w = csv.writer(f, lineterminator='\n')
        w.writerow(['build', 'workload', 'trial', 'iterations', 'process_cpu_ns', 'cpu_ns_per_call'])
        w.writerows(cpu)
    memory = []
    order = [(b, c, t) for b in builds for c in cases for t in range(5)]
    random.Random(202611057).shuffle(order)
    for b, (n, original, hot), t in order:
        elapsed, rss = run(builds[b], n, original)
        memory.append([b, n, t, rss])
    with (args.output / 'rss.csv').open('w') as f:
        w = csv.writer(f, lineterminator='\n')
        w.writerow(['build', 'workload', 'trial', 'peak_rss_bytes'])
        w.writerows(memory)
    d = collections.defaultdict(list)
    m = collections.defaultdict(list)
    for b, n, t, count, elapsed, each in cpu:
        d[b, n].append(each / 1e6)
    for b, n, t, rss in memory:
        m[b, n].append(rss / 2**20)
    lines = ['| Workload | Arc ms | Combined inline-4 ms | Lazy view-3 ms |', '| --- | ---: | ---: | ---: |']
    rss_lines = ['| Workload | Arc MiB | Combined inline-4 MiB | Lazy view-3 MiB |', '| --- | ---: | ---: | ---: |']
    for n, original, hot in cases:
        lines.append('| ' + n + ' | ' + ' | '.join(f'{statistics.median(d[b, n]):.3f}' for b in builds) + ' |')
        rss_lines.append('| ' + n + ' | ' + ' | '.join(f'{statistics.median(m[b, n]):.2f}' for b in builds) + ' |')
    (args.output / 'results.md').write_text('\n'.join(lines) + '\n')
    (args.output / 'rss.md').write_text('\n'.join(rss_lines) + '\n')
    print('\n'.join(lines), flush=True)
    print('\n'.join(rss_lines), flush=True)
    provenance = dict(
        builds={b: str(p) for b, p in builds.items()},
        executable_sha256={b: hashlib.sha256(p.read_bytes()).hexdigest() for b, p in builds.items()},
        cases=[
            dict(
                name=n,
                original=str(original),
                hot=str(hot),
                iterations=counts[n],
                input_sha256=hashlib.sha256((original / f'{n}.input').read_bytes()).hexdigest(),
            )
            for n, original, hot in cases
        ],
        trials=5,
        profile='profiling',
        revisions=dict(
            arc='f29350439dc1a5d5f63e7c0d30555edcf170a57c',
            inline4='61728c1c3d96c8094b72da76c043ab934372168e',
            lazy3='uncommitted optional view-cache extension on 61728c1c',
        ),
        method='macOS wait4 per-child user+system CPU / guest repetitions; randomized fresh processes; separate one-execution absolute RSS peaks',
    )
    (args.output / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')


if __name__ == '__main__':
    main()
