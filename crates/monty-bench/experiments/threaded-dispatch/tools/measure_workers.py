"""Flying Circus repeated-worker protocol, with contention checks per sample."""

import argparse
import hashlib
import json
import os
import platform
import random
import re
import statistics
import subprocess
import sys
import time
from contextlib import ExitStack
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('out', type=Path)
p.add_argument('variants', nargs='+')
p.add_argument('--cpython', type=Path, required=True)
p.add_argument('--flying-circus-source', type=Path, required=True)
p.add_argument('--samples', type=int, default=30)
p.add_argument('--warmups', type=int, default=5)
p.add_argument(
    '--allow-contention', action='store_true', help='Exploratory only; retain contention markers instead of retrying'
)
p.add_argument('--only', nargs='*')
a = p.parse_args()
# Use the explicitly selected Flying Circus checkout for both adapters and statistics.
sys.path.insert(0, str(a.flying_circus_source.resolve()))
from flying_circus.workers import CPythonWorker, MontyWorker  # noqa: E402
from flying_circus.workload_source import prepare, split_source  # noqa: E402

experiment = Path(__file__).resolve().parents[1]
a.out.mkdir(parents=True, exist_ok=False)
variants = dict(v.split('=', 1) for v in a.variants)
variants.setdefault('cpython', str(a.cpython.resolve()))
workloads = sorted((experiment / 'mixes').glob('*.py'))
workloads += sorted(experiment.glob('*.py'))
apps = a.flying_circus_source / 'flying_circus/workloads'
workloads += [
    apps / f'{name}.py'
    for name in (
        'expense_report',
        'order_cleanup',
        'api_report',
        'capacity_planning',
        'portfolio_risk',
        'ticket_search',
        'game_fannkuch',
        'game_nbody',
        'game_spectral_norm',
        'perf_float',
        'perf_unpack_sequence',
        'perf_richards',
    )
]
if a.only:
    workloads = [w for w in workloads if w.stem in a.only]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


metadata = {
    'command': sys.argv,
    'platform': platform.platform(),
    'started': time.time(),
    'samples': a.samples,
    'warmups': a.warmups,
    'binaries': {k: {'path': v, 'sha256': digest(v)} for k, v in variants.items()},
    'sources': {w.stem: {'path': str(w), 'sha256': digest(w)} for w in workloads},
    'harness_sha256': digest(__file__),
    'worker_adapter_sha256': digest(a.flying_circus_source / 'flying_circus/workers.py'),
    'scope': 'FC repeated worker: configure and fixture setup untimed; source compile/run and IPC timed; validation and reset untimed',
    'contention': 'process activity checked before and after every sample; rejected attempts retained',
    'exploratory_allow_contention': a.allow_contention,
}
(a.out / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
(a.out / 'harness.py').write_text(Path(__file__).read_text())
source_dir = a.out / 'sources'
source_dir.mkdir()
for workload in workloads:
    (source_dir / workload.name).write_text(workload.read_text())
activity, rows = [], []
own_pids = {os.getpid()}


def busy():
    active = []
    for line in subprocess.check_output(['ps', '-axo', 'pid,%cpu,comm'], text=True).splitlines()[1:]:
        fields = line.strip().split(None, 2)
        if len(fields) != 3:
            continue
        pid, cpu, command = int(fields[0]), float(fields[1]), fields[2]
        if pid not in own_pids and (
            re.search(r'/(rustc|cargo|llvm-objdump)$', command)
            or (
                cpu > 5
                and re.search(
                    r'(monty|access_cost|opcode_cost|candidate-access|baseline-access|flying-circus)', command
                )
            )
        ):
            active.append(line.strip())
    activity.append({'time': time.time(), 'active': active})
    (a.out / 'activity.json').write_text(json.dumps(activity, indent=2) + '\n')
    return active


def idle(name):
    if a.allow_contention:
        busy()
        return
    waits = 0
    while busy():
        if waits % 30 == 0:
            print(f'Waiting before {name}', flush=True)
        time.sleep(2)
        waits += 2


rng = random.Random(0)
with ExitStack() as stack:
    workers = {}
    for label, binary in variants.items():
        cls = CPythonWorker if label == 'cpython' else MontyWorker
        worker = stack.enter_context(cls(Path(binary), 60))
        worker.ready()
        # Protocol negotiation can replace the initial worker process.
        own_pids.add(worker.process.pid)
        workers[label] = worker
    for workload in workloads:
        source = workload.read_text()
        setup, code = split_source(source)
        # The same independent CPython oracle and exact-output validation as FC custom workloads.
        reference = subprocess.run([variants['cpython'], str(workload)], capture_output=True, text=True, check=True)
        if reference.stderr:
            raise ValueError(reference.stderr)
        expected = reference.stdout
        for sample in range(-a.warmups, a.samples):
            labels = list(workers)
            rng.shuffle(labels)
            for label in labels:
                while True:
                    idle(workload.stem)
                    worker = workers[label]
                    worker.configure(workload.name)
                    prepare(worker, setup, workload.name)
                    start = time.perf_counter_ns()
                    stdout, stderr = worker.run(code, workload.name)
                    elapsed = time.perf_counter_ns() - start
                    if stdout != expected or stderr:
                        raise ValueError(f'{label}/{workload.stem}: stdout={stdout!r}, stderr={stderr!r}')
                    worker.reset()
                    contended = bool(busy())
                    row = {
                        'workload': workload.stem,
                        'runtime': label,
                        'sample': sample,
                        'ns': elapsed,
                        'accepted': not contended or a.allow_contention,
                        'contended': contended,
                        'time': time.time(),
                    }
                    rows.append(row)
                    with (a.out / 'samples.jsonl').open('a') as f:
                        f.write(json.dumps(row) + '\n')
                    if not contended or a.allow_contention:
                        break
        medians = {
            label: statistics.median(
                r['ns']
                for r in rows
                if r['accepted'] and r['sample'] >= 0 and r['workload'] == workload.stem and r['runtime'] == label
            )
            / 1e6
            for label in workers
        }
        print(workload.stem, medians, flush=True)

summary = {}
for workload in workloads:
    summary[workload.stem] = {
        label: statistics.median(
            r['ns']
            for r in rows
            if r['accepted'] and r['sample'] >= 0 and r['workload'] == workload.stem and r['runtime'] == label
        )
        / 1e6
        for label in workers
    }
(a.out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
