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
import time
from pathlib import Path

fixtures = Path('/private/tmp/monty-splitlines-fixtures')
hot = Path('/private/tmp/monty-splitlines-hot')
hot.mkdir(exist_ok=True)
names = [
    'ndjson_ascii_splitlines',
    'ndjson_utf8_splitlines',
    'splitlines_bulk_ascii',
    'splitlines_bulk_utf8',
    'splitlines_keep_one',
    'splitlines_inline',
]
manifest = json.loads((fixtures / 'manifest.json').read_text())
selected = [v for v in manifest if v['name'] in names]
for entry in selected:
    name = entry['name']
    tree = ast.parse((fixtures / f'{name}.py').read_text())
    assert isinstance(tree.body[-1], ast.Expr)
    count = 100 if name.startswith('ndjson') or name.endswith('inline') else 10
    body = tree.body[:-1] + [ast.Return(tree.body[-1].value)]
    function = ast.FunctionDef(
        name='workload',
        args=ast.arguments(posonlyargs=[], args=[ast.arg(arg='data')], kwonlyargs=[], kw_defaults=[], defaults=[]),
        body=body,
        decorator_list=[],
    )
    loop = ast.parse(
        f'for iteration in range({count}):\n    result = workload(data)\n    assert result == {entry["expected"]}'
    ).body
    module = ast.fix_missing_locations(
        ast.Module(body=[function, *loop, ast.Expr(ast.Name(id='result', ctx=ast.Load()))], type_ignores=[])
    )
    (hot / f'{name}.py').write_text(ast.unparse(module) + '\n')
    target = hot / f'{name}.input'
    if target.exists():
        target.unlink()
    os.link(fixtures / f'{name}.input', target)
    entry['iterations'] = count
    (hot / 'manifest.json').write_text(json.dumps([{**v, 'retain': False} for v in selected], indent=2) + '\n')
exe = {
    'arcstr_copy': '/private/tmp/monty-rss-candidate',
    'arcstr_views': '/private/tmp/monty-rss-arcstr_views',
    'owned_copy': '/private/tmp/monty-rss-owned',
    'owned_views': '/private/tmp/monty-rss-owned_views',
}


def run(build, name, directory):
    env = dict(os.environ, MONTY_RSS_CASE=name, MONTY_RSS_FIXTURES=str(directory))
    with tempfile.TemporaryFile(mode='w+') as output:
        start = time.perf_counter_ns()
        child = subprocess.Popen(
            [exe[build], 'measure_peak_rss', '--ignored', '--test-threads=1'],
            env=env,
            stdout=output,
            stderr=subprocess.STDOUT,
        )
        _, status, usage = os.wait4(child.pid, 0)
        child.returncode = os.waitstatus_to_exitcode(status)
        wall = time.perf_counter_ns() - start
        if child.returncode:
            output.seek(0)
            raise RuntimeError(output.read())
    return int((usage.ru_utime + usage.ru_stime) * 1e9), wall, usage.ru_maxrss


for build in exe:
    run(build, 'empty', fixtures)
rows = []
order = [(b, e, t) for b in exe for e in selected for t in range(5)]
random.Random(20261105).shuffle(order)
for build, entry, trial in order:
    cpu, wall, rss = run(build, entry['name'], hot)
    rows.append(
        [
            build,
            entry['name'],
            trial,
            entry['iterations'],
            cpu,
            wall,
            cpu / entry['iterations'],
            wall / entry['iterations'],
        ]
    )
with Path('/private/tmp/monty-splitlines-cpu.csv').open('w') as f:
    w = csv.writer(f, lineterminator='\n')
    w.writerow(
        [
            'build',
            'workload',
            'trial',
            'iterations',
            'process_cpu_ns',
            'process_wall_ns',
            'cpu_ns_per_call',
            'wall_ns_per_call',
        ]
    )
    w.writerows(rows)
rssrows = []
order = [(b, n, t) for b in exe for n in ['empty', *names] for t in range(5)]
random.Random(20261106).shuffle(order)
for build, name, trial in order:
    cpu, wall, rss = run(build, name, fixtures)
    rssrows.append([build, name, trial, rss])
with Path('/private/tmp/monty-splitlines-rss.csv').open('w') as f:
    w = csv.writer(f, lineterminator='\n')
    w.writerow(['build', 'workload', 'trial', 'peak_rss_bytes'])
    w.writerows(rssrows)
d = collections.defaultdict(list)
m = collections.defaultdict(list)
for b, n, t, it, cpu, wall, each, w in rows:
    d[b, n].append(each)
for b, n, t, rss in rssrows:
    m[b, n].append(rss)
lines = [
    '| Workload | Arc<str> copy | Arc<str> views | Arc<String> copy | Arc<String> views |',
    '| --- | ---: | ---: | ---: | ---: |',
]
for n in names:
    lines.append('| ' + n + ' | ' + ' | '.join(f'{statistics.median(d[b, n]) / 1e6:.3f} ms' for b in exe) + ' |')
Path('/private/tmp/monty-splitlines-cpu-table.md').write_text('\n'.join(lines) + '\n')
print('\n'.join(lines), flush=True)
lines = [
    '| Workload | Arc<str> copy | Arc<str> views | Arc<String> copy | Arc<String> views |',
    '| --- | ---: | ---: | ---: | ---: |',
]
for n in ['empty', *names]:
    lines.append('| ' + n + ' | ' + ' | '.join(f'{statistics.median(m[b, n]) / 2**20:.2f} MiB' for b in exe) + ' |')
Path('/private/tmp/monty-splitlines-rss-table.md').write_text('\n'.join(lines) + '\n')
print('\n'.join(lines), flush=True)
provenance = {
    'production_builds': exe,
    'executable_sha256': {k: hashlib.sha256(Path(v).read_bytes()).hexdigest() for k, v in exe.items()},
    'trials_per_workload_per_build': 5,
    'timing_method': 'Per-child macOS wait4 user+system CPU divided by guest loop count; includes amortized startup/compile/teardown. Repeated calls share VM JSON caches.',
    'rss_method': 'One fresh process executing the original fixture once; macOS wait4 ru_maxrss bytes; absolute peak.',
    'input_sha256': {
        v['name']: hashlib.sha256((fixtures / (v['name'] + '.input')).read_bytes()).hexdigest() for v in selected
    },
    'cases': selected,
}
Path('/private/tmp/monty-splitlines-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
