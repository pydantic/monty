import ast
import collections
import csv
import json
import os
import random
import statistics
import subprocess
import tempfile
from pathlib import Path

root = Path('/private/tmp/monty-lazy-view-cache')
hot = root / 'hash-confirmation'
hot.mkdir(exist_ok=True)
old = Path('/private/tmp/monty-inline-benchmark/hot')
manifest = json.loads((old / 'manifest.json').read_text())
names = {'hash_1_views': 1000, 'hash_4_views': 300}
for name, count in names.items():
    tree = ast.parse((old / (name + '.py')).read_text())
    loop = next(n for n in tree.body if isinstance(n, ast.For))
    loop.iter.args = [ast.Constant(count)]
    (hot / (name + '.py')).write_text(ast.unparse(ast.fix_missing_locations(tree)) + '\n')
    (hot / (name + '.input')).write_bytes((old / (name + '.input')).read_bytes())
(hot / 'manifest.json').write_text(json.dumps([e for e in manifest if e['name'] in names]))
builds = {'inline4': '/private/tmp/monty-rss-dual_adaptive', 'lazy3': '/private/tmp/monty-rss-lazy_views'}
rows = []
order = [(b, n, t) for b in builds for n in names for t in range(5)]
random.Random(942).shuffle(order)
for b, n, t in order:
    with tempfile.TemporaryFile(mode='w+') as out:
        child = subprocess.Popen(
            [builds[b], 'measure_peak_rss', '--ignored', '--test-threads=1'],
            env=dict(os.environ, MONTY_RSS_CASE=n, MONTY_RSS_FIXTURES=str(hot)),
            stdout=out,
            stderr=subprocess.STDOUT,
        )
        _, s, u = os.wait4(child.pid, 0)
        child.returncode = os.waitstatus_to_exitcode(s)
        if child.returncode:
            out.seek(0)
            raise RuntimeError(out.read())
    rows.append([b, n, t, names[n], (u.ru_utime + u.ru_stime) * 1e9 / names[n]])
with (root / 'hash-confirmation.csv').open('w') as f:
    w = csv.writer(f, lineterminator='\n')
    w.writerow(['build', 'workload', 'trial', 'iterations', 'cpu_ns_per_call'])
    w.writerows(rows)
d = collections.defaultdict(list)
for b, n, t, c, cpu in rows:
    d[b, n].append(cpu / 1e6)
for n in names:
    a = statistics.median(d['inline4', n])
    b = statistics.median(d['lazy3', n])
    print(n, a, b, (b / a - 1) * 100, flush=True)
