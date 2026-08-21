"""Parse 500 log lines with a regex and aggregate two ways.

Monty's `re` is close to CPython's but not identical: there is no `VERBOSE` flag, so a
commented multi-line pattern fails, and `re.sub` rejects a callable replacement. Neither
is needed to solve this — but both are what a model reaches for when a pattern gets long
enough to want explaining, which is precisely when a 500-line log makes it long.
"""

from __future__ import annotations

from evals.harness.task import Exact, Task

_SERVICES = ['auth', 'billing', 'search', 'ingest']
_LEVELS = ['INFO', 'INFO', 'WARN', 'INFO', 'ERROR', 'INFO', 'ERROR', 'DEBUG']


def _build_log() -> str:
    """Deterministic log fixture: 500 lines, mixed levels, per-request durations."""
    lines: list[str] = []
    for index in range(500):
        service = _SERVICES[index % len(_SERVICES)]
        level = _LEVELS[index % len(_LEVELS)]
        duration = (index * 37) % 900 + 10
        request = f'req-{index:04d}'
        lines.append(
            f'2026-08-21T09:{index // 60:02d}:{index % 60:02d}Z {level:5} [{service}] {request} handled in {duration}ms'
        )
    return '\n'.join(lines)


_LOG = _build_log()


async def read_log() -> str:
    """Host function: the whole application log as text."""
    return _LOG


STUBS = '''
async def read_log() -> str:
    """Return the application log as one string.

    Each line looks like:
    `2026-08-21T09:04:12Z ERROR [billing] req-0251 handled in 431ms`
    """
    ...
'''


def _expected() -> dict[str, object]:
    errors: dict[str, int] = {}
    durations: list[tuple[int, str]] = []
    for index in range(500):
        service = _SERVICES[index % len(_SERVICES)]
        level = _LEVELS[index % len(_LEVELS)]
        duration = (index * 37) % 900 + 10
        if level == 'ERROR':
            errors[service] = errors.get(service, 0) + 1
        durations.append((duration, f'req-{index:04d}'))
    slowest = sorted(durations, key=lambda pair: (-pair[0], pair[1]))[:5]
    return {
        'errors_by_service': errors,
        'slowest': [{'request': request, 'ms': ms} for ms, request in slowest],
    }


REFERENCE = """
import re

text = await read_log()
pattern = re.compile(r'^\\S+ (\\w+) +\\[(\\w+)\\] (\\S+) handled in (\\d+)ms$')

errors = {}
durations = []
for line in text.splitlines():
    match = pattern.match(line)
    if match is None:
        continue
    level = match.group(1)
    service = match.group(2)
    request = match.group(3)
    ms = int(match.group(4))
    if level == 'ERROR':
        errors[service] = errors.get(service, 0) + 1
    durations.append({'request': request, 'ms': ms})

ordered = sorted(durations, key=lambda row: (-row['ms'], row['request']))

{'errors_by_service': errors, 'slowest': ordered[:5]}
"""

TASK = Task(
    name='log_parse',
    category='text',
    prompt=(
        'Parse the application log. Return a dict with "errors_by_service" (a dict of '
        'service name to the number of ERROR lines for it) and "slowest" (the 5 slowest '
        'requests as a list of dicts with "request" and "ms", slowest first, breaking '
        'ties by request id ascending).'
    ),
    stubs=STUBS,
    tools={'read_log': read_log},
    expected=Exact(_expected()),
    reference_solution=REFERENCE,
    traps=('re.VERBOSE', 're.sub with a callable', 'str.format'),
    expected_external_calls=1,
)
