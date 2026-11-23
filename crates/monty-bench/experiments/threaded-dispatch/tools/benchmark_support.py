"""Input validation and contention checks shared by the measurement scripts."""

import argparse
import math
import re
import subprocess
import time
from pathlib import Path


def parse_variants(parser, specifications):
    """Reject labels that collide or cannot safely form an output filename."""
    variants = {}
    labels = set()
    for specification in specifications:
        label, separator, executable = specification.partition('=')
        if not separator or not executable or not re.fullmatch(r'[A-Za-z0-9_-]+', label):
            parser.error(f'Expected label=path with an alphanumeric, underscore or hyphen label: {specification!r}')
        if label.lower() in labels:
            parser.error(f'Duplicate variant label: {label}')
        if label.lower() == 'cpython':
            parser.error('cpython is reserved for the CPython control')
        labels.add(label.lower())
        variants[label] = str(Path(executable).resolve())
    return variants


def positive_seconds(value):
    """Require a finite timeout so contention cannot cause an unbounded wait."""
    seconds = float(value)
    if not math.isfinite(seconds) or seconds <= 0:
        raise argparse.ArgumentTypeError('must be a finite positive number')
    return seconds


def competing_processes(own_pids):
    """Match both macOS paths and Linux comm names, which can be truncated to 15 characters."""
    active = []
    raw = subprocess.check_output(['ps', '-axo', 'pid,%cpu,comm'], text=True)
    for line in raw.splitlines()[1:]:
        fields = line.strip().split(None, 2)
        if len(fields) != 3:
            continue
        pid, cpu, command = int(fields[0]), float(fields[1]), fields[2]
        name = command.rsplit('/', 1)[-1]
        if pid not in own_pids and (
            name in ('rustc', 'cargo', 'llvm-objdump')
            or cpu > 5
            and re.search(r'(monty|access_cost|opcode_cost|candidate-acces|baseline-access|flying-circus)', name)
        ):
            active.append(line.strip())
    return active


def wait_for_idle(busy, timeout, name):
    """Fail with the blocking processes when the machine does not become idle."""
    deadline = time.monotonic() + timeout
    next_notice = 0
    while active := busy():
        now = time.monotonic()
        if now >= deadline:
            raise TimeoutError(f'Timed out waiting before {name}: {active}')
        if now >= next_notice:
            print(f'Waiting before {name}: {active}', flush=True)
            next_notice = now + 30
        time.sleep(min(2, deadline - now))
