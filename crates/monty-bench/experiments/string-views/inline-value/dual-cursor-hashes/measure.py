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

root = Path('/private/tmp/monty-dual-benchmark')
root.mkdir(exist_ok=True)
fixtures = Path('/private/tmp/monty-inline-benchmark/fixtures')
hot = Path('/private/tmp/monty-inline-benchmark/hot')
entries = {e['name']: e for e in json.loads((hot / 'manifest.json').read_text())}
for indexed in [False, True]:
    name = 'unicode_owners_' + ('indexed' if indexed else 'unindexed')
    source = 'lines = [str(i) + data for i in range(100000)]\n'
    source += 'total = 0\nfor line in lines:\n    total += ord(line[20])\ntotal\n' if indexed else 'len(lines)\n'
    text = 'é' * 80
    expected = 23300000 if indexed else 100000
    (fixtures / f'{name}.py').write_text(source)
    (fixtures / f'{name}.input').write_text(text)
    e = dict(name=name, expected=expected, retain=True, input_bytes=len(text.encode()), iterations=3)
    entries[name] = e
    tree = ast.parse(source)
    body = tree.body[:-1] + [ast.Return(tree.body[-1].value)]
    fn = ast.FunctionDef(
        name='workload',
        args=ast.arguments(posonlyargs=[], args=[ast.arg(arg='data')], kwonlyargs=[], kw_defaults=[], defaults=[]),
        body=body,
        decorator_list=[],
    )
    loop = ast.parse(f'for iteration in range(3):\n    result = workload(data)\n    assert result == {expected}').body
    module = ast.fix_missing_locations(
        ast.Module(body=[fn, *loop, ast.Expr(ast.Name(id='result', ctx=ast.Load()))], type_ignores=[])
    )
    (hot / f'{name}.py').write_text(ast.unparse(module) + '\n')
    (hot / f'{name}.input').write_text(text)
(hot / 'manifest.json').write_text(json.dumps([{**e, 'retain': False} for e in entries.values()]))
original = {e['name']: e for e in json.loads((fixtures / 'manifest.json').read_text())}
for n, e in entries.items():
    original.setdefault(n, e)
(fixtures / 'manifest.json').write_text(json.dumps(list(original.values())))
names = [
    'ndjson_ascii_tail',
    'ndjson_utf8_tail',
    'ndjson_utf8_splitlines',
    'prefix_remove_ascii',
    'prefix_remove_utf8',
    'suffix_remove_utf8',
    'splitlines_bulk_ascii',
    'splitlines_bulk_utf8',
    'splitlines_inline',
    'json_request',
    'template_fragments',
    'docx_text',
    'hash_1_views',
    'hash_4_views',
    'hash_8_views',
    'hash_64_views',
    'unicode_owners_unindexed',
    'unicode_owners_indexed',
]
selected = [entries[n] for n in names]
exe = {
    'arc': '/private/tmp/monty-rss-owned_views',
    'previous_inline': '/private/tmp/monty-rss-inline_fixed',
    'dual_adaptive': '/private/tmp/monty-rss-dual_adaptive',
}


def run(b, n, directory):
    env = dict(os.environ, MONTY_RSS_CASE=n, MONTY_RSS_FIXTURES=str(directory))
    with tempfile.TemporaryFile(mode='w+') as output:
        child = subprocess.Popen(
            [exe[b], 'measure_peak_rss', '--ignored', '--test-threads=1'],
            env=env,
            stdout=output,
            stderr=subprocess.STDOUT,
        )
        _, status, u = os.wait4(child.pid, 0)
        child.returncode = os.waitstatus_to_exitcode(status)
        if child.returncode:
            output.seek(0)
            raise RuntimeError(f'{b} {n} ' + output.read())
    return (u.ru_utime + u.ru_stime) * 1e9, u.ru_maxrss


for b in exe:
    for n in names:
        run(b, n, fixtures)
    print(b, 'correctness passed', flush=True)
rows = []
order = [(b, e, t) for b in exe for e in selected for t in range(5)]
random.Random(3366).shuffle(order)
for index, (b, e, t) in enumerate(order):
    cpu, rss = run(b, e['name'], hot)
    rows.append([b, e['name'], t, cpu / e['iterations']])
    if index % 40 == 0:
        print('CPU', index, '/', len(order), flush=True)
with (root / 'cpu.csv').open('w') as f:
    w = csv.writer(f, lineterminator='\n')
    w.writerow(['build', 'workload', 'trial', 'cpu_ns_per_call'])
    w.writerows(rows)
rssnames = [
    'ndjson_utf8_tail',
    'splitlines_bulk_ascii',
    'splitlines_bulk_utf8',
    'hash_64_views',
    'unicode_owners_unindexed',
    'unicode_owners_indexed',
]
rssrows = []
order = [(b, n, t) for b in exe for n in rssnames for t in range(5)]
random.Random(2233).shuffle(order)
for b, n, t in order:
    cpu, rss = run(b, n, fixtures)
    rssrows.append([b, n, t, rss])
with (root / 'rss.csv').open('w') as f:
    w = csv.writer(f, lineterminator='\n')
    w.writerow(['build', 'workload', 'trial', 'peak_rss_bytes'])
    w.writerows(rssrows)
d = collections.defaultdict(list)
for b, n, t, ns in rows:
    d[b, n].append(ns)
lines = [
    '| Workload | Arc ms | Previous inline ms | Dual/adaptive ms | New vs Arc |',
    '| --- | ---: | ---: | ---: | ---: |',
]
for n in names:
    values = [statistics.median(d[b, n]) / 1e6 for b in exe]
    lines.append(
        '| ' + n + ' | ' + ' | '.join(f'{v:.3f}' for v in values) + f' | {(values[2] / values[0] - 1) * 100:+.1f}% |'
    )
(root / 'results.md').write_text('\n'.join(lines) + '\n')
print('\n'.join(lines), flush=True)
m = collections.defaultdict(list)
for b, n, t, rss in rssrows:
    m[b, n].append(rss)
lines = ['| Workload | Arc MiB | Previous inline MiB | Dual/adaptive MiB |', '| --- | ---: | ---: | ---: |']
for n in rssnames:
    lines.append('| ' + n + ' | ' + ' | '.join(f'{statistics.median(m[b, n]) / 2**20:.2f}' for b in exe) + ' |')
(root / 'rss.md').write_text('\n'.join(lines) + '\n')
print('\n'.join(lines), flush=True)
(root / 'provenance.json').write_text(
    json.dumps(
        dict(
            executables=exe,
            sha256={b: hashlib.sha256(Path(p).read_bytes()).hexdigest() for b, p in exe.items()},
            cases=selected,
            input_sha256={
                e['name']: hashlib.sha256((fixtures / (e['name'] + '.input')).read_bytes()).hexdigest()
                for e in selected
            },
            trials=5,
            rss_trials=5,
            method='randomized fresh processes; macOS wait4 CPU / guest repetitions; separate original single-execution peak RSS',
        ),
        indent=2,
    )
)
