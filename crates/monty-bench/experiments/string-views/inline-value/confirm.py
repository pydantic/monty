import collections
import csv
import json
import os
import random
import statistics
import subprocess
import tempfile
from pathlib import Path

root = Path('/private/tmp/monty-inline-benchmark')
hot = root / 'hot'
entries = {e['name']: e for e in json.loads((hot / 'manifest.json').read_text())}
names = [
    'hash_4_views',
    'hash_8_views',
    'hash_64_views',
    'prefix_remove_utf8',
    'ndjson_utf8_tail',
    'splitlines_inline',
    'splitlines_bulk_ascii',
]
exe = {'arc_views': '/private/tmp/monty-rss-owned_views', 'inline_views': '/private/tmp/monty-rss-inline_views'}
rows = []
order = [(b, n, t) for b in exe for n in names for t in range(7)]
random.Random(737).shuffle(order)
for b, n, t in order:
    env = dict(os.environ, MONTY_RSS_CASE=n, MONTY_RSS_FIXTURES=str(hot))
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
            raise RuntimeError(output.read())
    rows.append([b, n, t, (u.ru_utime + u.ru_stime) * 1e9 / entries[n]['iterations']])
with (root / 'confirmation.csv').open('w') as f:
    w = csv.writer(f, lineterminator='\n')
    w.writerow(['build', 'workload', 'trial', 'cpu_ns_per_call'])
    w.writerows(rows)
d = collections.defaultdict(list)
for b, n, t, v in rows:
    d[b, n].append(v)
for n in names:
    a = statistics.median(d['arc_views', n]) / 1e6
    i = statistics.median(d['inline_views', n]) / 1e6
    print(f'{n}: Arc {a:.3f} ms; inline {i:.3f} ms; {i / a:.2f}x', flush=True)
