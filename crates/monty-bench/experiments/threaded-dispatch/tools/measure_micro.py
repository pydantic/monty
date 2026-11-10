import argparse
import csv
import hashlib
import json
import os
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('out', type=Path)
p.add_argument('variants', nargs='+')
p.add_argument('--rounds', type=int, default=3)
p.add_argument('--kind', choices=['access', 'nop'], default='access')
a = p.parse_args()
a.out.mkdir(exist_ok=False, parents=True)
variants = [v.split('=', 1) for v in a.variants]
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
activity = []
accepted = []


def busy():
    raw = subprocess.check_output(['ps', '-axo', 'pid,etime,%cpu,comm'], text=True)
    active = []
    for line in raw.splitlines()[1:]:
        fields = line.strip().split(None, 3)
        if len(fields) != 4:
            continue
        pid, elapsed, cpu, command = fields
        if int(pid) == os.getpid():
            continue
        if re.search(r'/(rustc|cargo|llvm-objdump)$', command) or (
            float(cpu) > 5
            and re.search(r'(monty|access_cost|opcode_cost|candidate-access|baseline-access|flying-circus)', command)
        ):
            active.append(line.strip())
    activity.append({'time': time.time(), 'active': active})
    (a.out / 'activity.json').write_text(json.dumps(activity, indent=2) + '\n')
    return active


for round_idx in range(a.rounds):
    order = variants if round_idx % 2 == 0 else list(reversed(variants))
    for label, exe in order:
        attempt = 0
        while True:
            waiting = 0
            while busy():
                if waiting % 30 == 0:
                    print(f'Waiting for idle machine before {label}, round {round_idx}', flush=True)
                time.sleep(2)
                waiting += 2
            path = a.out / f'{label}-{a.kind}-{round_idx}-attempt-{attempt}.csv'
            started = time.time()
            with path.open('w') as f:
                subprocess.run([exe], stdout=f, env={**os.environ, 'SAMPLES': '30'}, check=True)
            competing = busy()
            if competing:
                print(f'Keeping but excluding contended attempt {path.name}', flush=True)
                attempt += 1
                continue
            accepted.append(
                {'label': label, 'round': round_idx, 'path': str(path), 'started': started, 'finished': time.time()}
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
