"""Serve a scripted stream of HTTP requests the way a Cloudflare Worker would.

The sandbox is the worker: it pulls requests with `next_request`, routes them, talks
to the origin and a KV store through host functions, and answers with
`send_response`. The host replays thirty requests and records every response, so the
evaluator can check routing, caching, the admin gate and the per-IP rate limit
against what the rules say each response should have been.
"""

from __future__ import annotations

import asyncio
from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.evaluators import Predicate
from evals.harness.task import Task

RATE_LIMIT = 5
WINDOW = 60
ADMIN_TOKEN = 'letmein'
ORIGIN_LATENCY = 0.005

ORIGIN: dict[tuple[str, str], tuple[int, str]] = {
    ('GET', '/api/users'): (200, '[{"id": 1, "name": "Ada"}, {"id": 2, "name": "Grace"}]'),
    ('GET', '/api/orders'): (200, '[{"id": 10, "total": 42.5}]'),
    ('POST', '/api/orders'): (201, '{"id": 11}'),
    ('GET', '/static/app.js'): (200, 'console.log("app")'),
    ('GET', '/static/style.css'): (200, 'body { margin: 0 }'),
}


def _request(
    rid: int, method: str, path: str, ip: str, ts: int, headers: dict[str, str] | None = None
) -> dict[str, Any]:
    return {'id': rid, 'method': method, 'path': path, 'ip': ip, 'ts': ts, 'headers': headers or {}}


REQUESTS: list[dict[str, Any]] = [
    _request(1, 'GET', '/api/users', '10.0.0.1', 1000),
    _request(2, 'GET', '/static/app.js', '10.0.0.2', 1001),
    _request(3, 'GET', '/static/app.js', '10.0.0.3', 1002),
    _request(4, 'GET', '/admin', '10.0.0.1', 1003),
    _request(5, 'GET', '/admin', '10.0.0.1', 1004, {'x-admin-token': ADMIN_TOKEN}),
    _request(6, 'GET', '/nowhere', '10.0.0.2', 1005),
    _request(7, 'POST', '/api/orders', '10.0.0.4', 1006),
    _request(8, 'GET', '/api/orders', '10.0.0.4', 1007),
    _request(9, 'GET', '/static/style.css', '10.0.0.5', 1008),
    _request(10, 'GET', '/static/style.css', '10.0.0.6', 1009),
    _request(11, 'GET', '/api/missing', '10.0.0.7', 1010),
    # 10.0.0.9 makes eight requests inside one window: the last three are over the limit.
    *[_request(12 + i, 'GET', '/api/users', '10.0.0.9', 1011 + i) for i in range(8)],
    _request(20, 'GET', '/static/app.js', '10.0.0.9', 1080),  # a new window, allowed again
    _request(21, 'GET', '/api/users', '10.0.0.1', 1081),
    _request(22, 'GET', '/static/missing.js', '10.0.0.1', 1082),
    _request(23, 'GET', '/static/missing.js', '10.0.0.1', 1083),
    _request(24, 'DELETE', '/api/users', '10.0.0.1', 1084),
    _request(25, 'GET', '/admin', '10.0.0.8', 1085, {'x-admin-token': 'wrong'}),
    _request(26, 'GET', '/api/orders', '10.0.0.8', 1086),
    _request(27, 'GET', '/', '10.0.0.8', 1087),
    _request(28, 'GET', '/static/app.js', '10.0.0.8', 1088),
    _request(29, 'GET', '/api/users', '10.0.0.2', 1089),
    _request(30, 'GET', '/api/users', '10.0.0.2', 1090),
]


def _expected_responses() -> dict[int, dict[str, Any]]:
    """What the rules in the prompt say each request should get."""
    counters: dict[str, int] = {}
    cache: set[str] = set()
    out: dict[int, dict[str, Any]] = {}
    for req in REQUESTS:
        window_key = f'{req["ip"]}:{req["ts"] // WINDOW}'
        counters[window_key] = counters.get(window_key, 0) + 1
        if counters[window_key] > RATE_LIMIT:
            out[req['id']] = {'status': 429, 'cache': None}
            continue
        path = req['path']
        if path.startswith('/api/'):
            status, _ = ORIGIN.get((req['method'], path), (404, 'not found'))
            out[req['id']] = {'status': status, 'cache': None}
        elif path.startswith('/static/'):
            if path in cache:
                out[req['id']] = {'status': 200, 'cache': 'HIT'}
            else:
                status, _ = ORIGIN.get(('GET', path), (404, 'not found'))
                if status == 200:
                    cache.add(path)
                out[req['id']] = {'status': status, 'cache': 'MISS'}
        elif path == '/admin':
            ok = req['headers'].get('x-admin-token') == ADMIN_TOKEN
            out[req['id']] = {'status': 200 if ok else 403, 'cache': None}
        else:
            out[req['id']] = {'status': 404, 'cache': None}
    return out


EXPECTED_RESPONSES = _expected_responses()

_queue: list[dict[str, Any]] = []
_kv: dict[str, Any] = {}
RESPONSES: dict[int, dict[str, Any]] = {}
ORIGIN_CALLS: list[tuple[str, str]] = []


def _reset() -> None:
    _queue[:] = list(REQUESTS)
    _kv.clear()
    RESPONSES.clear()
    ORIGIN_CALLS.clear()


async def next_request() -> dict[str, Any] | None:
    """Host function: the next request, or `None` when the stream is drained."""
    return _queue.pop(0) if _queue else None


async def fetch_origin(method: str, path: str, headers: dict[str, str] | None = None) -> dict[str, Any]:
    """Host function: forward to the origin server."""
    await asyncio.sleep(ORIGIN_LATENCY)
    ORIGIN_CALLS.append((method, path))
    status, body = ORIGIN.get((method, path), (404, 'not found'))
    return {'status': status, 'headers': {'content-type': 'application/json'}, 'body': body}


def kv_get(key: str) -> Any:
    """Host function: read a KV value, `None` when missing."""
    return _kv.get(key)


def kv_put(key: str, value: Any) -> None:
    """Host function: write a KV value."""
    _kv[key] = value


async def send_response(request_id: int, status: int, headers: dict[str, str], body: str) -> None:
    """Host function: deliver the response for a request."""
    RESPONSES[request_id] = {'status': status, 'headers': dict(headers), 'body': body}


def _responses_ok(_result: object) -> bool:
    """Every request answered as the rules demand, cache hits served without the origin."""
    if set(RESPONSES) != set(EXPECTED_RESPONSES):
        return False
    for rid, want in EXPECTED_RESPONSES.items():
        got = RESPONSES[rid]
        if got['status'] != want['status']:
            return False
        if got['headers'].get('x-proxy') != 'monty':
            return False
        if want['cache'] is not None and got['headers'].get('x-cache') != want['cache']:
            return False
    static_fetches = [path for _, path in ORIGIN_CALLS if path.startswith('/static/')]
    hits = sum(1 for want in EXPECTED_RESPONSES.values() if want['cache'] == 'HIT')
    static_total = sum(1 for want in EXPECTED_RESPONSES.values() if want['cache'] is not None)
    return len(static_fetches) == static_total - hits


STUBS = '''
from typing import Any

async def next_request() -> dict[str, Any] | None:
    """The next incoming request, or `None` when there are no more.

    A request has `id` (int), `method`, `path`, `ip`, `ts` (unix seconds, int) and `headers` (lowercase keys).
    """
    ...

async def fetch_origin(method: str, path: str, headers: dict[str, str] | None = None) -> dict[str, Any]:
    """Forward a request to the origin; returns `status`, `headers` and `body`."""
    ...

def kv_get(key: str) -> Any:
    """Read from the worker's KV store; `None` when the key is missing."""
    ...

def kv_put(key: str, value: Any) -> None:
    """Write to the worker's KV store."""
    ...

async def send_response(request_id: int, status: int, headers: dict[str, str], body: str) -> None:
    """Deliver the response for a request. Every request must get exactly one."""
    ...
'''

PROMPT = f"""
You are the edge worker in front of an origin server. Pull requests with `next_request()` until it returns `None`
and answer each one with `send_response`. Rules, in this order:

1. Rate limit: allow at most {RATE_LIMIT} requests per client `ip` per {WINDOW}-second window (`ts // {WINDOW}`),
   counting in KV; a request over the limit gets 429 with body `rate limited` and nothing else happens for it.
2. `/api/...`: forward to the origin with `fetch_origin` and relay its status and body.
3. `/static/...`: cache the origin's 200 responses in KV under the path; serve repeats from KV with header
   `x-cache: HIT`, and origin fetches with `x-cache: MISS` (a 404 is not cached).
4. `/admin`: 200 with body `admin ok` when header `x-admin-token` is `{ADMIN_TOKEN}`, otherwise 403.
5. Anything else: 404.

Every response carries the header `x-proxy: monty`. Return the number of requests you answered.
"""

REFERENCE = """
handled = 0
while True:
    req = await next_request()
    if req is None:
        break
    handled += 1
    headers = {'x-proxy': 'monty'}
    window = req['ts'] // 60
    counter_key = f"rate:{req['ip']}:{window}"
    seen = kv_get(counter_key) or 0
    kv_put(counter_key, seen + 1)
    if seen + 1 > 5:
        await send_response(req['id'], 429, headers, 'rate limited')
        continue
    path = req['path']
    if path.startswith('/api/'):
        upstream = await fetch_origin(req['method'], path, req['headers'])
        await send_response(req['id'], upstream['status'], headers, upstream['body'])
    elif path.startswith('/static/'):
        cached = kv_get('cache:' + path)
        if cached is not None:
            headers['x-cache'] = 'HIT'
            await send_response(req['id'], 200, headers, cached)
        else:
            upstream = await fetch_origin('GET', path)
            headers['x-cache'] = 'MISS'
            if upstream['status'] == 200:
                kv_put('cache:' + path, upstream['body'])
            await send_response(req['id'], upstream['status'], headers, upstream['body'])
    elif path == '/admin':
        if req['headers'].get('x-admin-token') == 'letmein':
            await send_response(req['id'], 200, headers, 'admin ok')
        else:
            await send_response(req['id'], 403, headers, 'forbidden')
    else:
        await send_response(req['id'], 404, headers, 'not found')
handled
"""

TASK = Task(
    name='reverse_proxy',
    category='web',
    prompt=PROMPT.strip(),
    stubs=STUBS,
    tools={
        'next_request': next_request,
        'fetch_origin': fetch_origin,
        'kv_get': kv_get,
        'kv_put': kv_put,
        'send_response': send_response,
    },
    expected=len(REQUESTS),
    evaluators=(
        EqualsExpected(),
        Predicate('every response matches the routing, cache and rate-limit rules', _responses_ok),
    ),
    reference_solution=REFERENCE,
    traps=('rate limiting after routing', 'caching 404s', 'forgetting the loop terminator'),
    max_result_bytes=20,
    setup=_reset,
)
