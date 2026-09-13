"""A 2 MB log answered by delegating chunks to `rlm_query`, the recursive RLM case.

`context` is far bigger than `support_tickets`, so the shape is chunk, fan out, merge:
the code slices the log, asks `rlm_query` to count error codes per host in each
chunk, gathers the answers and merges them. Under `--dry-run` the child is the
deterministic `_stub_rlm_query`, a sub-model call over the chunk rather than a child
REPL of its own.
"""

from __future__ import annotations

import asyncio
import json
import math
import random

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

_HOSTS = ('host-1', 'host-2', 'host-3', 'host-4')
_SERVICES = ('billing', 'search', 'auth', 'ingest')
_CODES = ('E11', 'E23', 'E42', 'E57', 'E88')
_LEVELS = ('INFO',) * 6 + ('WARN',) * 2 + ('ERROR',) * 2
_LINES = 24_000
_CHUNK = 100_000


def _build(seed: int) -> tuple[str, dict[str, dict[str, int]]]:
    """The log and the true `{code: {host: count}}` for ERROR lines."""
    rng = random.Random(seed)
    counts: dict[str, dict[str, int]] = {}
    lines: list[str] = []
    for index in range(_LINES):
        level = rng.choice(_LEVELS)
        host = rng.choice(_HOSTS)
        service = rng.choice(_SERVICES)
        minute, second = divmod(index // 4, 60)
        stamp = f'2026-08-21T{(9 + minute // 60) % 24:02d}:{minute % 60:02d}:{second:02d}Z'
        if level == 'ERROR':
            code = rng.choice(_CODES)
            counts.setdefault(code, {}).setdefault(host, 0)
            counts[code][host] += 1
            lines.append(f'{stamp} {host} ERROR svc={service} code={code} msg=request failed after retries')
        else:
            lines.append(f'{stamp} {host} {level} svc={service} msg=request handled in {rng.randrange(5, 900)}ms')
    return '\n'.join(lines) + '\n', counts


def _answer(counts: dict[str, dict[str, int]]) -> dict[str, object] | None:
    """The expected dict, or `None` when a ranking is tied."""
    totals = sorted(((sum(hosts.values()), code) for code, hosts in counts.items()), reverse=True)
    if totals[0][0] == totals[1][0]:
        return None
    count, code = totals[0]
    hosts = sorted(((n, host) for host, n in counts[code].items()), reverse=True)
    if hosts[0][0] == hosts[1][0]:
        return None
    return {'code': code, 'count': count, 'host': hosts[0][1], 'host_count': hosts[0][0]}


def _pick() -> tuple[str, dict[str, object]]:
    for seed in range(20260901, 20261001):
        context, counts = _build(seed)
        answer = _answer(counts)
        if answer is not None:
            return context, answer
    raise RuntimeError('no seed produced an unambiguous answer')


CONTEXT, EXPECTED = _pick()


def _stub_rlm_query(prompt: str, chunk: str) -> str:
    """Dry-run child: count ERROR lines by code and host in `chunk`, as JSON."""
    counts: dict[str, dict[str, int]] = {}
    for line in chunk.split('\n'):
        if ' ERROR ' not in line:
            continue
        parts = line.split(' ')
        host = parts[1]
        code = next((p[len('code=') :] for p in parts if p.startswith('code=')), '?')
        counts.setdefault(code, {}).setdefault(host, 0)
        counts[code][host] += 1
    return json.dumps(counts)


async def rlm_query(prompt: str, chunk: str) -> str:
    """Host: ask a child model about `chunk`; the stub answers after a short latency so gathers overlap."""
    await asyncio.sleep(0.005)
    return _stub_rlm_query(prompt, chunk)


STUBS = '''
context: str
"""The complete application log for one day. About 2 MB; do not print it."""

async def rlm_query(prompt: str, chunk: str) -> str:
    """Ask a child model `prompt` about `chunk`, a slice of the log, and return its reply.

    The child sees only `chunk`; keep chunks under about 150,000 characters. Ask for
    JSON and it will reply with JSON.
    """
    ...
'''

REFERENCE = """
import asyncio
import json

size = 100000
chunks = []
start = 0
while start < len(context):
    end = context.find('\\n', start + size)
    if end == -1:
        end = len(context)
    chunks.append(context[start:end])
    start = end + 1

prompt = (
    'Count the ERROR lines in this log chunk by error code and host. Reply with JSON of the '
    'form {"<code>": {"<host>": <count>}} and nothing else.'
)
replies = await asyncio.gather(*[rlm_query(prompt, chunk) for chunk in chunks])

merged = {}
for reply in replies:
    for code, hosts in json.loads(reply).items():
        for host, n in hosts.items():
            by_host = merged.setdefault(code, {})
            by_host[host] = by_host.get(host, 0) + n

code = sorted(merged, key=lambda c: sum(merged[c].values()), reverse=True)[0]
host = sorted(merged[code], key=lambda h: merged[code][h], reverse=True)[0]
{'code': code, 'count': sum(merged[code].values()), 'host': host, 'host_count': merged[code][host]}
"""

_CHUNKS = math.ceil(len(CONTEXT) / _CHUNK)

TASK = Task(
    name='nested_rlm',
    category='agents',
    prompt=(
        'The variable `context` is a full day of application logs, about 2 MB, one line per '
        'entry like `2026-08-21T09:12:03Z host-3 ERROR svc=billing code=E42 msg=...`. It is too '
        'large to read, and you should not parse it all yourself: split it into chunks and ask '
        '`rlm_query` to count the ERROR lines by error code and host in each chunk, then merge '
        'the counts in code. Which error code occurred most often, and which host produced the '
        'most of that code? Return a dict with "code", "count", "host" and "host_count".'
    ),
    stubs=STUBS,
    tools={'rlm_query': rlm_query},
    inputs={'context': CONTEXT},
    expected=EXPECTED,
    evaluators=(ApproxExpected(),),
    reference_solution=REFERENCE,
    traps=('slicing a 2 MB string', 'gathering child calls', 'merging nested counts'),
    # One child per chunk of about 100 KB, all gathered at once; two waves are still fine.
    expected_call_batches=2,
    max_result_bytes=200,
)
