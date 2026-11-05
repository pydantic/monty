"""Generate mixed-width UTF-8 sliding windows and compare frozen production builds."""

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


def expected_result(source, data):
    tree = ast.parse(source)
    result = tree.body.pop()
    namespace = {'data': data}
    exec(compile(tree, 'strides.py', 'exec'), namespace)
    return eval(compile(ast.Expression(result.value), 'strides.py', 'eval'), namespace)


def prepare(directory):
    fixtures = directory / 'fixtures'
    hot = directory / 'hot'
    fixtures.mkdir(parents=True, exist_ok=True)
    hot.mkdir(exist_ok=True)
    entries = []
    bases = {'latin': 'éñüøλçåä', 'cjk': '東京漢字仮名文章', 'mixed': 'aé中🐍bø字🌍'}
    for encoding, base in bases.items():
        data = ''.join(base * 8 + f'{i:08x}\n' for i in range(4096))[:131072]
        assert len(data) == 131072
        assert not data.isascii()
        for stride in [1, 8, 64, 1024]:
            name = f'{encoding}_windows_stride_{stride}'
            source = f"""width = 65536
limit = len(data) - width + 1
total = 0
for i in range(2048):
    start = (i * {stride}) % limit
    window = data[start:start + width]
    total += len(window) + ord(window[0]) + ord(window[width // 2]) + ord(window[-1])
total
"""
            entries.append(
                dict(
                    name=name,
                    source=source,
                    data=data,
                    iterations=5,
                    retain=False,
                    encoding=encoding,
                    width=65536,
                    stride=stride,
                    slice_step=1,
                    windows=2048,
                )
            )
        name = f'{encoding}_small_windows'
        source = """width = 4096
limit = len(data) - width + 1
total = 0
for i in range(2048):
    start = (i * 64) % limit
    window = data[start:start + width]
    total += len(window) + ord(window[0]) + ord(window[width // 2]) + ord(window[-1])
total
"""
        entries.append(
            dict(
                name=name,
                source=source,
                data=data,
                iterations=5,
                retain=False,
                encoding=encoding,
                width=4096,
                stride=64,
                slice_step=1,
                windows=2048,
            )
        )
        name = f'{encoding}_stepped_windows'
        source = """width = 65536
limit = len(data) - width + 1
total = 0
for i in range(128):
    start = (i * 64) % limit
    window = data[start:start + width:2]
    total += len(window) + ord(window[0]) + ord(window[len(window) // 2]) + ord(window[-1])
total
"""
        entries.append(
            dict(
                name=name,
                source=source,
                data=data,
                iterations=3,
                retain=False,
                encoding=encoding,
                width=65536,
                stride=64,
                slice_step=2,
                windows=128,
            )
        )
        name = f'{encoding}_retained_windows'
        source = """width = 65536
limit = len(data) - width + 1
windows = [data[(i * 256) % limit:(i * 256) % limit + width] for i in range(256)]
data = None
total = 0
for window in windows:
    total += len(window) + ord(window[0]) + ord(window[width // 2]) + ord(window[-1])
total
"""
        entries.append(
            dict(
                name=name,
                source=source,
                data=data,
                iterations=2,
                retain=True,
                encoding=encoding,
                width=65536,
                stride=256,
                slice_step=1,
                windows=256,
            )
        )
        name = f'{encoding}_index_only'
        source = """width = 65536
limit = len(data) - width + 1
total = 0
for i in range(2048):
    start = (i * 1024) % limit
    total += ord(data[start]) + ord(data[start + width // 2]) + ord(data[start + width - 1])
total
"""
        entries.append(
            dict(
                name=name,
                source=source,
                data=data,
                iterations=5,
                retain=False,
                encoding=encoding,
                width=65536,
                stride=1024,
                slice_step=None,
                windows=0,
            )
        )
    manifest = []
    for entry in entries:
        source = entry.pop('source')
        data = entry.pop('data')
        entry['expected'] = expected_result(source, data)
        entry['input_bytes'] = len(data.encode())
        entry['input_characters'] = len(data)
        entry['input_sha256'] = hashlib.sha256(data.encode()).hexdigest()
        name = entry['name']
        (fixtures / f'{name}.py').write_text(source)
        (fixtures / f'{name}.input').write_text(data)
        tree = ast.parse(source)
        body = tree.body[:-1] + [ast.Return(tree.body[-1].value)]
        function = ast.FunctionDef(
            name='workload',
            args=ast.arguments(posonlyargs=[], args=[ast.arg(arg='data')], kwonlyargs=[], kw_defaults=[], defaults=[]),
            body=body,
            decorator_list=[],
        )
        loop = ast.parse(
            f'for repeat in range({entry["iterations"]}):\n    result = workload(data)\n    assert result == {entry["expected"]}'
        ).body
        module = ast.fix_missing_locations(
            ast.Module(body=[function, *loop, ast.Expr(ast.Name(id='result', ctx=ast.Load()))], type_ignores=[])
        )
        (hot / f'{name}.py').write_text(ast.unparse(module) + '\n')
        (hot / f'{name}.input').write_text(data)
        manifest.append(entry)
    (fixtures / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (hot / 'manifest.json').write_text(json.dumps([{**e, 'retain': False} for e in manifest], indent=2) + '\n')
    return manifest, fixtures, hot


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
    parser.add_argument('--output', type=Path, default=Path('/private/tmp/monty-utf8-strides'))
    parser.add_argument('--main', type=Path, default=Path('/private/tmp/monty-rss-strides_main'))
    parser.add_argument('--index', type=Path, default=Path('/private/tmp/monty-rss-base'))
    parser.add_argument('--optimized', type=Path, default=Path('/private/tmp/monty-rss-dual_adaptive'))
    parser.add_argument('--prepare-only', action='store_true')
    parser.add_argument('--trials', type=int, default=5)
    args = parser.parse_args()
    entries, fixtures, hot = prepare(args.output)
    print(f'{len(entries)} CPython expected-result checks passed', flush=True)
    if args.prepare_only:
        return
    builds = dict(main=args.main, index_pr=args.index, optimized=args.optimized)
    for build, executable in builds.items():
        for entry in entries:
            run(executable, entry['name'], fixtures)
        print(build, 'all result checks passed', flush=True)
    cpu_rows = []
    order = [(build, e, trial) for build in builds for e in entries for trial in range(args.trials)]
    random.Random(202611051).shuffle(order)
    for i, (build, entry, trial) in enumerate(order):
        cpu, rss = run(builds[build], entry['name'], hot)
        cpu_rows.append([build, entry['name'], trial, entry['iterations'], cpu, cpu / entry['iterations']])
        if i % 40 == 0:
            print('CPU', i, '/', len(order), flush=True)
    with (args.output / 'cpu.csv').open('w') as f:
        writer = csv.writer(f, lineterminator='\n')
        writer.writerow(['build', 'workload', 'trial', 'iterations', 'process_cpu_ns', 'cpu_ns_per_call'])
        writer.writerows(cpu_rows)
    rss_rows = []
    order = [(build, e['name'], trial) for build in builds for e in entries for trial in range(args.trials)]
    random.Random(202611052).shuffle(order)
    for build, name, trial in order:
        cpu, rss = run(builds[build], name, fixtures)
        rss_rows.append([build, name, trial, rss])
    with (args.output / 'rss.csv').open('w') as f:
        writer = csv.writer(f, lineterminator='\n')
        writer.writerow(['build', 'workload', 'trial', 'peak_rss_bytes'])
        writer.writerows(rss_rows)
    timing = collections.defaultdict(list)
    memory = collections.defaultdict(list)
    for build, name, trial, iterations, cpu, each in cpu_rows:
        timing[build, name].append(each)
    for build, name, trial, rss in rss_rows:
        memory[build, name].append(rss)
    lines = [
        '| Workload | Main ms | Index PR ms | Optimized ms | Speedup vs main | Speedup vs index PR |',
        '| --- | ---: | ---: | ---: | ---: | ---: |',
    ]
    rss_lines = ['| Workload | Main MiB | Index PR MiB | Optimized MiB |', '| --- | ---: | ---: | ---: |']
    for e in entries:
        name = e['name']
        values = [statistics.median(timing[b, name]) / 1e6 for b in builds]
        lines.append(
            '| '
            + name
            + ' | '
            + ' | '.join(f'{v:.3f}' for v in values)
            + f' | {values[0] / values[2]:.2f}x | {values[1] / values[2]:.2f}x |'
        )
        rss_lines.append(
            '| ' + name + ' | ' + ' | '.join(f'{statistics.median(memory[b, name]) / 2**20:.2f}' for b in builds) + ' |'
        )
    (args.output / 'results.md').write_text('\n'.join(lines) + '\n')
    (args.output / 'rss.md').write_text('\n'.join(rss_lines) + '\n')
    print('\n'.join(lines), flush=True)
    print('\n'.join(rss_lines), flush=True)
    provenance = dict(
        builds={k: str(v) for k, v in builds.items()},
        executable_sha256={k: hashlib.sha256(v.read_bytes()).hexdigest() for k, v in builds.items()},
        machine=platform.platform(),
        profile='profiling',
        trials=args.trials,
        cases=entries,
        revisions=dict(
            main='2a3269043e544d0e7bd99e3e5cf0c306a8246076',
            index_pr='5c9dddf39c92981b8ef023be059e8c3aeabf7e91',
            optimized='uncommitted inline views with two cursors and adaptive hashes on f29350439dc1a5d5f63e7c0d30555edcf170a57c',
        ),
        timing_method='randomized fresh processes; macOS wait4 user+system CPU / guest repetitions, includes amortized startup/compile/teardown',
        rss_method='absolute macOS wait4 peak RSS bytes; separate fresh processes executing original workload once',
    )
    (args.output / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')


if __name__ == '__main__':
    main()
