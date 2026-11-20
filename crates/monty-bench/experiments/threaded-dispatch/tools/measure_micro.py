import argparse
import csv
import hashlib
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

from benchmark_support import competing_processes, parse_variants, positive_seconds, wait_for_idle

p = argparse.ArgumentParser()
p.add_argument('out', type=Path)
p.add_argument('variants', nargs='+')
p.add_argument('--rounds', type=int, default=3)
p.add_argument('--kind', choices=['access', 'nop'], default='access')
p.add_argument('--idle-timeout', type=positive_seconds, default=300)
a = p.parse_args()
variants = list(parse_variants(p, a.variants).items())
if a.rounds <= 0:
    p.error('--rounds must be positive')
a.out.mkdir(exist_ok=False, parents=True)
metadata = {
    'command': sys.argv,
    'binaries': {
        label: {'path': exe, 'sha256': hashlib.sha256(Path(exe).read_bytes()).hexdigest()} for label, exe in variants
    },
    'samples': 30,
    'rounds': a.rounds,
    'contention': 'process activity checked before and after each executable; detected contention rejects the whole attempt',
}
(a.out / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
accepted = []


def busy():
    """Append process observations, retaining them if the run is interrupted."""
    active = competing_processes({os.getpid()})
    with (a.out / 'activity.jsonl').open('a') as log:
        log.write(json.dumps({'time': time.time(), 'active': active}) + '\n')
    return active


for round_idx in range(a.rounds):
    order = variants if round_idx % 2 == 0 else list(reversed(variants))
    for label, exe in order:
        attempt = 0
        while True:
            wait_for_idle(busy, a.idle_timeout, f'{label}, round {round_idx}')
            path = a.out / f'{label}-{a.kind}-{round_idx}-attempt-{attempt}.csv'
            started = time.time()
            with path.open('w') as f:
                subprocess.run([exe], stdout=f, env={**os.environ, 'SAMPLES': '30'}, check=True)
            competing = busy()
            if competing:
                print(f'Keeping but excluding contended attempt {path.name}', flush=True)
                attempt += 1
                if attempt >= 10:
                    raise RuntimeError(f'Ten contended attempts for {label}, round {round_idx}: {competing}')
                continue
            accepted.append(
                {'label': label, 'round': round_idx, 'path': path.name, 'started': started, 'finished': time.time()}
            )
            (a.out / 'accepted.json').write_text(json.dumps(accepted, indent=2) + '\n')
            groups = {}
            for row in csv.DictReader(path.open()):
                key = (row.get('case', row.get('nops')), row['interpreter'])
                groups.setdefault(key, []).append(float(row['ns']))
            print(
                label, round_idx, {str(k): round(statistics.median(v) / 1e6, 6) for k, v in groups.items()}, flush=True
            )
            break
