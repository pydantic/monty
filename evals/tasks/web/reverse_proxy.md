# reverse_proxy

The sandbox is a Cloudflare-Worker-shaped edge: it pulls thirty scripted requests with `next_request()` until
`None`, and answers each with `send_response`.
Rules: a per-IP limit of 5 requests per 60-second window counted in KV (429 beyond it), `/api/*` proxied through
`fetch_origin`, `/static/*` cached in KV with `x-cache: HIT` or `MISS` (404s not cached), `/admin` gated on an
`x-admin-token` header, everything else 404, and `x-proxy: monty` on every response.
The script includes eight requests from one IP in a window, repeated static paths, a missing static file and a bad
admin token.
Host functions: `next_request`, `fetch_origin`, `send_response` (async) and `kv_get`, `kv_put` (sync).

Scored with `EqualsExpected` on the number of requests answered, a `Predicate` comparing every recorded response's
status and headers to what the rules require and checking cache hits never reached the origin, and `result_size`.
