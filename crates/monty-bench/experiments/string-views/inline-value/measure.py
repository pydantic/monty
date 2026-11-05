import ast
import collections
import csv
import hashlib
import json
import os
import random
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

root = Path('/private/tmp/monty-inline-benchmark')
root.mkdir(exist_ok=True)
fixtures = root / 'fixtures'
hot = root / 'hot'
fixtures.mkdir(exist_ok=True)
hot.mkdir(exist_ok=True)
old = Path('/private/tmp/monty-splitlines-fixtures')
entries = json.loads((old / 'manifest.json').read_text())
for entry in entries:
    for suffix in ['py', 'input']:
        shutil.copyfile(old / f'{entry["name"]}.{suffix}', fixtures / f'{entry["name"]}.{suffix}')
for count in [1, 4, 8, 64]:
    name = f'hash_{count}_views'
    text = ''.join(chr(65 + i % 26) for i in range(16384))
    source = f'views = [data[i:i + 8192] for i in range({count})]\nexpected = [hash(v) for v in views]\nfor repeat in range(2000):\n    for i in range({count}):\n        assert hash(views[i]) == expected[i]\n{count}\n'
    (fixtures / f'{name}.py').write_text(source)
    (fixtures / f'{name}.input').write_text(text)
    entries.append(dict(name=name, expected=count, retain=False, input_bytes=len(text)))
for suffix, text in [('ascii', 'x' * 20000), ('utf8', 'é🌍' * 10000)]:
    name = f'prefix_remove_{suffix}'
    (fixtures / f'{name}.py').write_text('s = data\nwhile len(s) > 1:\n    s = s[1:]\nlen(s)\n')
    (fixtures / f'{name}.input').write_text(text)
    entries.append(dict(name=name, expected=1, retain=False, input_bytes=len(text.encode())))
(fixtures / 'manifest.json').write_text(json.dumps(entries))
for e in entries:
    name = e['name']
    tree = ast.parse((fixtures / f'{name}.py').read_text())
    count = (
        5
        if name.startswith('hash_')
        else 10
        if 'bulk' in name or 'keep_one' in name or 'prefix_remove' in name or name.endswith('_tail')
        else 40
    )
    e['iterations'] = count
    body = tree.body[:-1] + [ast.Return(tree.body[-1].value)]
    function = ast.FunctionDef(
        name='workload',
        args=ast.arguments(posonlyargs=[], args=[ast.arg(arg='data')], kwonlyargs=[], kw_defaults=[], defaults=[]),
        body=body,
        decorator_list=[],
    )
    loop = ast.parse(
        f'for iteration in range({count}):\n    result = workload(data)\n    assert result == {e["expected"]}'
    ).body
    module = ast.fix_missing_locations(
        ast.Module(body=[function, *loop, ast.Expr(ast.Name(id='result', ctx=ast.Load()))], type_ignores=[])
    )
    (hot / f'{name}.py').write_text(ast.unparse(module) + '\n')
    shutil.copyfile(fixtures / f'{name}.input', hot / f'{name}.input')
(hot / 'manifest.json').write_text(json.dumps([{**e, 'retain': False} for e in entries]))
if os.environ.get('PREPARE_ONLY'):
    raise SystemExit
exe = {
    'copy': '/private/tmp/monty-rss-base',
    'arc_views': '/private/tmp/monty-rss-owned_views',
    'inline_views': '/private/tmp/monty-rss-inline_views',
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
            raise RuntimeError(f'{build} {name}: ' + output.read())
    return int((usage.ru_utime + usage.ru_stime) * 1e9), wall, usage.ru_maxrss


for b in exe:
    for e in entries:
        run(b, e['name'], fixtures)
    print(b, 'all correctness checks passed', flush=True)
rows = []
order = [(b, e, t) for b in exe for e in entries for t in range(5)]
random.Random(20261105).shuffle(order)
for index, (b, e, t) in enumerate(order):
    cpu, wall, rss = run(b, e['name'], hot)
    rows.append([b, e['name'], t, e['iterations'], cpu, wall, cpu / e['iterations']])
    if index % 30 == 0:
        print('CPU', index, '/', len(order), flush=True)
with (root / 'cpu.csv').open('w') as f:
    w = csv.writer(f, lineterminator='\n')
    w.writerow(['build', 'workload', 'trial', 'iterations', 'cpu_ns', 'wall_ns', 'cpu_ns_per_call'])
    w.writerows(rows)
rssrows = []
order = [(b, e['name'], t) for b in exe for e in entries for t in range(5)]
random.Random(20261106).shuffle(order)
for b, n, t in order:
    cpu, wall, rss = run(b, n, fixtures)
    rssrows.append([b, n, t, rss])
with (root / 'rss.csv').open('w') as f:
    w = csv.writer(f, lineterminator='\n')
    w.writerow(['build', 'workload', 'trial', 'peak_rss_bytes'])
    w.writerows(rssrows)
d = collections.defaultdict(list)
m = collections.defaultdict(list)
for b, n, t, it, cpu, wall, each in rows:
    d[b, n].append(each)
for b, n, t, rss in rssrows:
    m[b, n].append(rss)
lines = [
    '| Workload | Copy ms | Arc ms | Inline ms | Inline vs Arc | Arc RSS MiB | Inline RSS MiB |',
    '| --- | ---: | ---: | ---: | ---: | ---: | ---: |',
]
for e in entries:
    n = e['name']
    a = statistics.median(d['arc_views', n])
    i = statistics.median(d['inline_views', n])
    lines.append(
        '| '
        + n
        + ' | '
        + ' | '.join(f'{statistics.median(d[b, n]) / 1e6:.3f}' for b in exe)
        + f' | {(i / a - 1) * 100:+.1f}% | {statistics.median(m["arc_views", n]) / 2**20:.2f} | {statistics.median(m["inline_views", n]) / 2**20:.2f} |'
    )
(root / 'results.md').write_text('\n'.join(lines) + '\n')
print('\n'.join(lines), flush=True)
(root / 'provenance.json').write_text(
    json.dumps(
        dict(
            executables=exe,
            sha256={k: hashlib.sha256(Path(v).read_bytes()).hexdigest() for k, v in exe.items()},
            cases=entries,
            method='5 randomized fresh processes; wait4 user+system CPU / guest repetitions; separate single-execution absolute peak RSS in bytes',
        ),
        indent=2,
    )
)
