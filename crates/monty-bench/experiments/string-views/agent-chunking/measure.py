"""Compare agent-style multilingual document chunking, with calibrated batches."""

import argparse
import ast
import collections
import csv
import hashlib
import json
import os
import platform
import random
import statistics
import subprocess
import tempfile
from pathlib import Path

from prepare import prepare

HERE = Path(__file__).resolve().parent


def write_batch(entry, directory, original, count):
    tree = ast.parse(original)
    function = ast.FunctionDef(
        name='workload',
        args=ast.arguments(posonlyargs=[], args=[ast.arg(arg='data')], kwonlyargs=[], kw_defaults=[], defaults=[]),
        body=tree.body[:-1] + [ast.Return(tree.body[-1].value)],
        decorator_list=[],
    )
    loop = ast.parse(
        f'for iteration in range({count}):\n    result = workload(data)\n    assert result == {entry["expected"]}'
    ).body
    module = ast.fix_missing_locations(
        ast.Module(body=[function, *loop, ast.Expr(ast.Name(id='result', ctx=ast.Load()))], type_ignores=[])
    )
    (directory / f'{entry["name"]}.py').write_text(ast.unparse(module) + '\n')


def run(command, name, directory):
    env = dict(os.environ, MONTY_RSS_CASE=name, MONTY_RSS_FIXTURES=str(directory))
    with tempfile.TemporaryFile(mode='w+') as output:
        child = subprocess.Popen(command, env=env, stdout=output, stderr=subprocess.STDOUT)
        _, status, usage = os.wait4(child.pid, 0)
        child.returncode = os.waitstatus_to_exitcode(status)
        if child.returncode:
            output.seek(0)
            raise RuntimeError(f'{name}: {output.read()}')
    return (usage.ru_utime + usage.ru_stime) * 1e9, usage.ru_maxrss


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('/private/tmp/monty-agent-chunking'))
    parser.add_argument('--python', type=Path, default=Path('/Users/jaheba/p/monty/.venv/bin/python'))
    parser.add_argument('--main', type=Path, default=Path('/private/tmp/monty-rss-strides_main'))
    parser.add_argument('--index', type=Path, default=Path('/private/tmp/monty-rss-base'))
    parser.add_argument('--optimized', type=Path, default=Path('/private/tmp/monty-rss-dual_adaptive'))
    parser.add_argument('--trials', type=int, default=5)
    parser.add_argument('--target-ms', type=int, default=500)
    args = parser.parse_args()
    entries = prepare(args.output)
    fixtures = args.output / 'fixtures'
    builds = {
        k: [str(p), 'measure_peak_rss', '--ignored', '--test-threads=1']
        for k, p in [('main', args.main), ('index_pr', args.index), ('optimized', args.optimized)]
    }
    builds['cpython'] = [str(args.python), str(HERE / 'cpython_runner.py')]
    counts = {}
    calibration = []
    directories = {}
    for build, command in builds.items():
        directory = args.output / ('batch-' + build)
        directory.mkdir(exist_ok=True)
        directories[build] = directory
        (directory / 'manifest.json').write_text(json.dumps(entries))
        for entry in entries:
            name = entry['name']
            (directory / f'{name}.input').write_text((fixtures / f'{name}.input').read_text())
            original = (fixtures / f'{name}.py').read_text()
            cpu, rss = run(command, name, fixtures)
            write_batch(entry, directory, original, 1)
            count = 1
            # Fast cases must amortize interpreter startup and input/compile costs on every build.
            for _ in range(3):
                elapsed, _ = run(command, name, directory)
                if elapsed >= args.target_ms * 1e6 * 0.8 or count >= 2048:
                    break
                count = min(2048, max(count + 1, int(count * args.target_ms * 1e6 / max(elapsed, 1))))
                write_batch(entry, directory, original, count)
            counts[build, name] = count
            calibration.append([build, name, count, elapsed, cpu, rss])
            print('calibrated', build, name, count, f'original {cpu / 1e6:.1f} ms', flush=True)
    rows = []
    order = [(b, e['name'], t) for b in builds for e in entries for t in range(args.trials)]
    random.Random(202611053).shuffle(order)
    for i, (build, name, trial) in enumerate(order):
        cpu, rss = run(builds[build], name, directories[build])
        count = counts[build, name]
        rows.append([build, name, trial, count, cpu, cpu / count])
        if i % 20 == 0:
            print('CPU', i, '/', len(order), flush=True)
    with (args.output / 'cpu.csv').open('w') as f:
        w = csv.writer(f, lineterminator='\n')
        w.writerow(['build', 'workload', 'trial', 'iterations', 'process_cpu_ns', 'cpu_ns_per_call'])
        w.writerows(rows)
    rssrows = []
    order = [(b, e['name'], t) for b in builds for e in entries for t in range(args.trials)]
    random.Random(202611054).shuffle(order)
    for build, name, trial in order:
        cpu, rss = run(builds[build], name, fixtures)
        rssrows.append([build, name, trial, rss, cpu])
    with (args.output / 'rss.csv').open('w') as f:
        w = csv.writer(f, lineterminator='\n')
        w.writerow(['build', 'workload', 'trial', 'peak_rss_bytes', 'single_execution_cpu_ns'])
        w.writerows(rssrows)
    d = collections.defaultdict(list)
    m = collections.defaultdict(list)
    for b, n, t, count, cpu, each in rows:
        d[b, n].append(each / 1e6)
    for b, n, t, rss, cpu in rssrows:
        m[b, n].append(rss / 2**20)
    lines = ['| Workload | Main ms | Index PR ms | Optimized ms | CPython ms |', '| --- | ---: | ---: | ---: | ---: |']
    memory = [
        '| Workload | Main MiB | Index PR MiB | Optimized MiB | CPython MiB |',
        '| --- | ---: | ---: | ---: | ---: |',
    ]
    for e in entries:
        n = e['name']
        lines.append('| ' + n + ' | ' + ' | '.join(f'{statistics.median(d[b, n]):.3f}' for b in builds) + ' |')
        memory.append('| ' + n + ' | ' + ' | '.join(f'{statistics.median(m[b, n]):.2f}' for b in builds) + ' |')
    (args.output / 'results.md').write_text('\n'.join(lines) + '\n')
    (args.output / 'rss.md').write_text('\n'.join(memory) + '\n')
    print('\n'.join(lines), flush=True)
    print('\n'.join(memory), flush=True)
    provenance = dict(
        builds=builds,
        executable_sha256={
            b: hashlib.sha256(Path(command[0]).read_bytes()).hexdigest() for b, command in builds.items()
        },
        cpython_version=subprocess.check_output([str(args.python), '--version'], text=True).strip(),
        machine=platform.platform(),
        trials=args.trials,
        target_batch_ms=args.target_ms,
        calibration=calibration,
        cases=entries,
        revisions=dict(
            main='2a3269043e544d0e7bd99e3e5cf0c306a8246076',
            index_pr='5c9dddf39c92981b8ef023be059e8c3aeabf7e91',
            optimized='uncommitted dual-cursor/adaptive-hash inline views on f29350439dc1a5d5f63e7c0d30555edcf170a57c',
        ),
        method='randomized fresh processes; wait4 user+system CPU divided by calibrated guest iterations; startup/compile/input/teardown amortized but included; RSS separate original single execution',
        source_sha256={p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in HERE.glob('*.py')},
    )
    (args.output / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')


if __name__ == '__main__':
    main()
