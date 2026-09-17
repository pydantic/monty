"""Score a site's agent-readiness in two waves of fetches: the AgentCon lighthouse audit.

Half the rubric depends on what an earlier response contained (the pages `llms.txt`
links to, the sitemap `robots.txt` names, the endpoint in `mcp.json`, SPF only when
MX records exist), so the natural shape is one gathered wave of known URLs, parsing in
Python, then one gathered wave of discovered URLs. `expected_call_batches=2` scores
exactly that.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.task import Task

DOMAIN = 'example.dev'
FETCH_LATENCY = 0.01

_HOMEPAGE = """<!doctype html>
<html><head>
<title>Example Dev</title>
<meta name="description" content="Tools for example developers.">
<meta property="og:title" content="Example Dev">
<meta property="og:description" content="Tools for example developers.">
<link rel="alternate" type="text/markdown" href="/index.md">
</head><body>
<main><h1>Example Dev</h1><p>Welcome.</p></main>
</body></html>
"""

_LLMS_TXT = """# Example Dev

- [Getting started](https://example.dev/docs/start)
- [API reference](https://example.dev/docs/api)
- [Changelog](https://example.dev/docs/changelog)
"""

_ROBOTS = """User-agent: *
Allow: /

User-agent: GPTBot
Disallow: /

Sitemap: https://example.dev/sitemap.xml
"""

_SITEMAP = """<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.dev/</loc></url>
  <url><loc>https://example.dev/docs/start</loc></url>
</urlset>
"""

_LONG_PAGE = '# Docs\n\n' + ('Example documentation paragraph with enough words to pass the size check. ' * 12)

SITE: dict[str, tuple[int, str, str]] = {
    'https://example.dev/': (200, 'text/html', _HOMEPAGE),
    'https://example.dev/llms.txt': (200, 'text/plain', _LLMS_TXT),
    'https://example.dev/docs/start': (200, 'text/html', f'<html><body><main>{_LONG_PAGE}</main></body></html>'),
    'https://example.dev/docs/api': (200, 'text/html', f'<html><body><main>{_LONG_PAGE}</main></body></html>'),
    'https://example.dev/index.md': (200, 'text/markdown', _LONG_PAGE),
    'https://example.dev/robots.txt': (200, 'text/plain', _ROBOTS),
    'https://example.dev/sitemap.xml': (200, 'application/xml', _SITEMAP),
    'https://example.dev/openapi.json': (200, 'application/json', json.dumps({'openapi': '3.1.0', 'paths': {}})),
    'https://example.dev/.well-known/mcp.json': (
        200,
        'application/json',
        json.dumps({'name': 'example', 'endpoint': 'https://example.dev/mcp'}),
    ),
    'https://example.dev/mcp': (200, 'application/json', json.dumps({'ok': True})),
}
DNS: dict[tuple[str, str], list[str]] = {
    (DOMAIN, 'A'): ['203.0.113.10'],
    (DOMAIN, 'AAAA'): [],
    (DOMAIN, 'MX'): ['10 mail.example.dev.'],
    (DOMAIN, 'TXT'): ['v=spf1 include:_spf.example.dev ~all'],
}

# The rubric applied to the fixture by hand: 12 + 10 + 10 + 5 + 5 + 5 + 5 + 5 + 5 + 0 + 5 + 5 + 5 + 5.
EXPECTED = {'score': 82, 'grade': 'B'}


async def fetch(url: str, accept: str | None = None) -> dict[str, Any]:
    """Host function: GET a URL from the fixture site; unknown paths are 404."""
    await asyncio.sleep(FETCH_LATENCY)
    status, content_type, body = SITE.get(url, (404, 'text/plain', 'not found'))
    if accept == 'text/markdown' and url == 'https://example.dev/':
        status, content_type, body = 200, 'text/markdown', _LONG_PAGE
    return {
        'url': url,
        'status': status,
        'final_url': url,
        'content_type': content_type,
        'body': body,
        'headers': {'content-type': content_type},
        'error': None,
    }


async def dns_lookup(domain: str, record_type: str) -> list[str]:
    """Host function: DNS records from the fixture; empty when there are none."""
    await asyncio.sleep(FETCH_LATENCY)
    return list(DNS.get((domain, record_type), []))


STUBS = '''
from typing import Any

async def fetch(url: str, accept: str | None = None) -> dict[str, Any]:
    """HTTP GET `url` (https only). Optional `accept` header value.

    Returns a dict with `url`, `status`, `final_url`, `content_type`, `body`, `headers`
    (lowercase keys) and `error` (set, with `status` 0, when the request failed).
    """
    ...

async def dns_lookup(domain: str, record_type: str) -> list[str]:
    """DNS records of `record_type` (A, AAAA, MX, TXT, CNAME, NS) for `domain`, as strings; empty if none."""
    ...
'''

PROMPT = """
Audit how ready `example.dev` is for AI agents and return `{"score": <int>, "grade": <letter>}`, printing a short
markdown findings table along the way. Score 100 points across these checks; award full, half or zero as stated.

A. Discovery (40)
1. `/llms.txt` (15): 5 if it returns 200 and is non-empty. Then extract every URL it lists (markdown links or bare
   `https://` lines), fetch up to 3 of them, and award the remaining 10 in proportion to how many return 200 with a
   body of at least 500 bytes (3/3 = 10, 2/3 = 7, 1/3 = 3, 0/3 = 0).
2. Markdown alternative (10): fetch the homepage and look for `<link rel="alternate" type="text/markdown" href=...>`;
   if present fetch that href, otherwise try `/llms-full.txt`, then the homepage again with `accept='text/markdown'`.
   10 if any of those returns 200 and looks like markdown (content type contains `markdown`, or body starts with `#`,
   or body lacks `<html`); 5 if 200 but it looks like HTML.
3. MCP (10): 5 if `/.well-known/mcp.json` (or `/mcp`) returns parseable JSON; 5 more if the endpoint URL it names
   (fields like `endpoint`, `url`, `server.url`) responds 200-299 to a GET.
4. OpenAPI (5): `/openapi.json` or `/.well-known/openapi.json` returns 200 and parses as JSON with an `openapi` or
   `swagger` field.

B. Crawlability (20)
5. `/robots.txt` exists (5).
6. `robots.txt` does not disallow GPTBot, ClaudeBot, anthropic-ai, PerplexityBot or Google-Extended (10 if none
   blocked, 5 if some, 0 if all).
7. Sitemap (5): parse `Sitemap:` directives from robots.txt, falling back to `/sitemap.xml`; fetch it, extract
   `<loc>` entries, sample up to 2 and check they return 200 (5 if both, 2.5 if one, 0 otherwise).

C. Homepage semantics (25)
8. `<title>` and `<meta name="description">` (5). 9. `og:title` and `og:description` (5).
10. At least one `application/ld+json` block declaring a schema.org type (10). 11. An `<h1>` and a `<main>` or
`<article>` (5).

D. Infrastructure (15)
12. `https://example.dev/` returns 200-399 (5). 13. An A or AAAA record resolves (5).
14. SPF conditional on MX (5): look up MX; with no MX records award 5; with MX records award 5 only if a TXT record
contains `v=spf1`.

Sum to a total and grade A >= 90, B >= 75, C >= 60, D >= 40, else F. Half-points round down in the total. Fetch
everything whose URL you already know in one `asyncio.gather` wave, parse in Python, then fetch the dependent URLs in a
second wave; do not fetch one URL at a time.
"""

REFERENCE = """
import asyncio
import json
import re
from typing import Any

base = 'https://example.dev'
known = [base + '/', base + '/llms.txt', base + '/robots.txt', base + '/sitemap.xml', base + '/openapi.json',
         base + '/.well-known/mcp.json', base + '/mcp', base + '/llms-full.txt', base + '/.well-known/openapi.json']
pages: list[Any] = await asyncio.gather(*[fetch(u) for u in known], dns_lookup('example.dev', 'A'),
                             dns_lookup('example.dev', 'AAAA'), dns_lookup('example.dev', 'MX'),
                             dns_lookup('example.dev', 'TXT'))
by_url = {page['url']: page for page in pages[:len(known)]}
dns_a, dns_aaaa, dns_mx, dns_txt = pages[len(known):]

home = by_url[base + '/']
llms = by_url[base + '/llms.txt']
robots = by_url[base + '/robots.txt']
sitemap_page = by_url[base + '/sitemap.xml']

llms_links = []
if llms['status'] == 200:
    llms_links = re.findall(r'https://[^\\s)>\\]]+', llms['body'])[:3]
alt = re.search(r'<link[^>]*rel="alternate"[^>]*type="text/markdown"[^>]*href="([^"]+)"', home['body'])
alt_url = (base + alt.group(1)) if alt and alt.group(1).startswith('/') else (alt.group(1) if alt else None)
mcp = by_url[base + '/.well-known/mcp.json']
mcp_doc = None
if mcp['status'] == 200:
    try:
        mcp_doc = json.loads(mcp['body'])
    except ValueError:
        mcp_doc = None
endpoint = None
if isinstance(mcp_doc, dict):
    endpoint = mcp_doc.get('endpoint') or mcp_doc.get('url')
    server = mcp_doc.get('server')
    if endpoint is None and isinstance(server, dict):
        endpoint = server.get('url')
sitemaps = re.findall(r'(?im)^sitemap:\\s*(\\S+)', robots['body']) if robots['status'] == 200 else []
sitemap_url = sitemaps[0] if sitemaps else base + '/sitemap.xml'

second = []
second_keys = []
for link in llms_links:
    second.append(fetch(link)); second_keys.append('llms:' + link)
if alt_url:
    second.append(fetch(alt_url)); second_keys.append('alt')
else:
    second.append(fetch(base + '/', accept='text/markdown')); second_keys.append('alt')
if endpoint:
    second.append(fetch(endpoint)); second_keys.append('mcp')
if sitemap_url != base + '/sitemap.xml':
    second.append(fetch(sitemap_url)); second_keys.append('sitemap')
locs = []
sm_body = sitemap_page['body'] if sitemap_url == base + '/sitemap.xml' and sitemap_page['status'] == 200 else ''
if sm_body:
    locs = re.findall(r'<loc>\\s*([^<\\s]+)\\s*</loc>', sm_body)[:2]
for loc in locs:
    second.append(fetch(loc)); second_keys.append('loc:' + loc)
answers: list[Any] = await asyncio.gather(*second)
results = dict(zip(second_keys, answers))

points = []
# 1
p1 = 5 if llms['status'] == 200 and llms['body'].strip() else 0
good = 0
for link in llms_links:
    r = results['llms:' + link]
    if r['status'] == 200 and len(r['body']) >= 500:
        good += 1
p1 += [0, 3, 7, 10][good] if llms_links else 0
points.append(p1)
# 2
md = results.get('alt')
if md is None or md['status'] != 200:
    md = by_url[base + '/llms-full.txt'] if by_url[base + '/llms-full.txt']['status'] == 200 else None
if md is None:
    points.append(0)
elif 'markdown' in md['content_type'] or md['body'].startswith('#') or '<html' not in md['body']:
    points.append(10)
else:
    points.append(5)
# 3
p3 = 5 if isinstance(mcp_doc, dict) else 0
probe = results.get('mcp')
if probe is not None and 200 <= probe['status'] < 300:
    p3 += 5
points.append(p3)
# 4
p4 = 0
for candidate in (by_url[base + '/openapi.json'], by_url[base + '/.well-known/openapi.json']):
    if candidate['status'] == 200:
        try:
            doc = json.loads(candidate['body'])
        except ValueError:
            doc = None
        if isinstance(doc, dict) and ('openapi' in doc or 'swagger' in doc):
            p4 = 5
points.append(p4)
# 5, 6
points.append(5 if robots['status'] == 200 else 0)
bots = ['GPTBot', 'ClaudeBot', 'anthropic-ai', 'PerplexityBot', 'Google-Extended']
blocked = 0
if robots['status'] == 200:
    for bot in bots:
        block = re.search(r'(?is)user-agent:\\s*' + re.escape(bot) + r'\\s*\\n\\s*disallow:\\s*/', robots['body'])
        if block:
            blocked += 1
points.append(10 if blocked == 0 else (0 if blocked == len(bots) else 5))
# 7
live = 0
for loc in locs:
    if results['loc:' + loc]['status'] == 200:
        live += 1
points.append(5 if live == 2 else (2.5 if live == 1 else 0))
# 8-11
body = home['body']
points.append(5 if '<title>' in body and 'name="description"' in body else 0)
points.append(5 if 'og:title' in body and 'og:description' in body else 0)
points.append(10 if 'application/ld+json' in body and 'schema.org' in body else 0)
points.append(5 if '<h1' in body and ('<main' in body or '<article' in body) else 0)
# 12-14
points.append(5 if 200 <= home['status'] < 400 else 0)
points.append(5 if dns_a or dns_aaaa else 0)
if not dns_mx:
    points.append(5)
else:
    points.append(5 if any('v=spf1' in rec for rec in dns_txt) else 0)

total = 0.0
for p in points:
    total = total + p
score = int(total)
grade = 'A' if score >= 90 else 'B' if score >= 75 else 'C' if score >= 60 else 'D' if score >= 40 else 'F'
print(f'# Agent-Readiness Report - example.dev\\n\\n**Score: {score}/100   Grade: {grade}**')
for i, p in enumerate(points):
    print(f'| {i + 1} | {p} |')
{'score': score, 'grade': grade}
"""

TASK = Task(
    name='lighthouse',
    category='web',
    prompt=PROMPT.strip(),
    stubs=STUBS,
    tools={'fetch': fetch, 'dns_lookup': dns_lookup},
    expected=EXPECTED,
    evaluators=(EqualsExpected(),),
    reference_solution=REFERENCE,
    traps=('fetching one URL per turn', 're.VERBOSE', 'html parsing without a parser'),
    expected_call_batches=2,
    max_result_bytes=60,
)
