# lighthouse

The AgentCon agent-readiness audit against a fixture site for `example.dev`: `llms.txt` linking three pages (one
404), a markdown alternate for the homepage, `mcp.json` naming an endpoint, `openapi.json`, `robots.txt` blocking
GPTBot and naming a sitemap with two entries, homepage HTML without JSON-LD, and DNS records including MX and an SPF
TXT.
The prompt carries the 14-check, 100-point rubric; the fixture scores 82, grade B.
Host functions: `fetch(url, accept)` and `dns_lookup(domain, record_type)`, both async with 10 ms of latency.

Scored with `EqualsExpected` on `{'score': 82, 'grade': 'B'}`, `within_call_budget` with a budget of 2 (one gathered
wave of known URLs, one of discovered ones), and `result_size`.
