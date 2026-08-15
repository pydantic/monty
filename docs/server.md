# Running monty-server

`monty-server` is a WebSocket server that hosts Monty sandbox workers: each connection gets its own `monty` worker
subprocess from an elastic pool.
A worker serves one session at a time and is reset or replaced between sessions, so no sandbox state crosses from one
client to another.
`pydantic_monty.AsyncMontyWebsocket` connects directly from Python; the TypeScript package (`@pydantic/monty`) is
subprocess-only today and cannot dial a remote server.
The server adds capacity limits, timeouts, per-caller quotas, health probes, graceful drain and tracing to [Pydantic
Logfire](https://pydantic.dev/logfire); the sandboxing itself is entirely the `monty` worker's.

The server is closed-source and distributed as a container image.
For access and licensing, [contact us](https://pydantic.dev/contact).

## Why Monty over WebSocket

This is not a sales document, but it's worth briefly enumerating why someone would want to use Monty running on a remote
server:

- **Security**: escaping the sandbox gets you the machine running Monty, not the machine running the agent / application
  code — and that machine is an empty container.
- **Centralized monitoring, observability, and scaling**: one horizontally scalable service for all Monty code
  execution, instead of every service running its own worker pool.
- **Density**: Monty workers have a small baseline footprint (as little as 2MB), plus additional memory for limits and
  optional type checking, so a single machine can run hundreds.
- **Same behavior as local Monty**: the wire protocol carries host callbacks, name lookups, async futures and mounted
  client directories, so code that runs against a local pool runs unchanged against the server.
- **(In future) switch to a full monty sandbox with a parameter**: the same interface, but a full VM running CPython —
  for code that needs dependencies, bash and a real filesystem.

## Quickstart

The image is private: commercial access comes with a `key.json` service-account file that authenticates pulls (the same
mechanism as self-hosted Logfire images, which live on the same registry host):

```bash
docker login us-docker.pkg.dev -u _json_key --password-stdin < key.json
```

On Kubernetes, the same file becomes an image pull secret referenced from the pod spec:

```bash
kubectl create secret docker-registry monty-image-key \
  --docker-server=us-docker.pkg.dev \
  --docker-username=_json_key \
  --docker-password="$(cat key.json)"
```

The image refuses to start without a dump-signing key of at least 16 bytes:

```bash
docker run --rm \
  -e MONTY_SERVER_DUMP_KEY="$(openssl rand -hex 16)" \
  -p 8000:8000 \
  us-docker.pkg.dev/pydantic-public-registries/monty/monty-server:latest
```

The bound URL (`ws://0.0.0.0:8000/`) is the only line written to stdout; all logging goes to stderr.
The image bundles the matching `monty` worker binary and runs as non-root uid 65532 on a `scratch` base — no shell, no
package manager.

## Connecting a client

```python test="skip"
from pydantic_monty import AsyncMontyWebsocket


async def main():
    async with AsyncMontyWebsocket('ws://localhost:8000/') as pool:
        async with pool.checkout() as session:
            print(await session.feed_run('1 + 1'))
            #> 2
```

An ordinary `GET /` on the same URL answers with a short info page.

## Configuration

Every flag has an environment variable: the flag name in upper snake case with a `MONTY_SERVER_` prefix
(`--max-sessions` → `MONTY_SERVER_MAX_SESSIONS`), except `MONTY_BIN` and `LOGFIRE_TOKEN`, which keep their established
names.
A flag on the command line wins over its variable.
Flags passed to `docker run <image> ...` replace the default `CMD` (`--host 0.0.0.0`) wholesale, so include `--host
0.0.0.0` when passing your own.
Bind to an IP literal, not a hostname — the `scratch` image has no name resolution.

The flags an operator usually sets first:

| Flag                            | Meaning                                        | Default     |
| ------------------------------- | ---------------------------------------------- | ----------- |
| `--host` / `--port`             | bind address; port 0 binds ephemeral           | 8000        |
| `--max-sessions`                | concurrent sessions across the server          | 64          |
| `--max-sessions-per-client`     | concurrent sessions per caller; 0 disables     | 10          |
| `--turn-timeout <secs>`         | wall-clock cap on a single turn                | 300         |
| `--idle-timeout <secs>`         | cap on the gap between requests                | 60          |
| `--session-timeout <secs>`      | cap on total session lifetime                  | 3600        |
| `--max-duration <secs>`         | sandbox execution time within a session        | 60          |
| `--max-memory-mib <MiB>`        | per-session memory ceiling                     | 64          |
| `--max-recursion-depth <n>`     | per-session call-stack ceiling                 | 1000        |
| `--dump-key <key>`              | required; signs session dumps                  | —           |
| `--logfire-token <token>`       | export traces to Logfire                       | off         |

Run `--help` for the full list.
Prefer environment variables for `--dump-key` and `--logfire-token`: a command line is world-readable via `ps`.

Each of the three resource limits is a ceiling.
A client that configures no value gets the server's; a client that asks for more is clamped to it; asking for less
always works.
The worker enforces the clamped value, so a `MemoryError` or duration error quotes the effective limit.

## Sizing the container

Each worker's process memory is capped at roughly `--max-memory-mib` + a small baseline + headroom (4 MiB, or 32 MiB
with type checking), so:

```
container memory ≈ --max-sessions × (--max-memory-mib + baseline + headroom)
```

The defaults (64 sessions × 64 MiB) reach several GiB.
Lower one of the two to fit your instance.

## Health probes and drain

`GET /health` answers 200 normally and 503 from the moment drain begins, so point your readiness probe there.
Point liveness at `GET /`, which answers regardless of drain — a readiness probe on `/` would keep a draining pod in
rotation, and a liveness probe on `/health` would restart one that is shutting down cleanly.

On SIGTERM the server drains: each session's next request is answered with a signed dump the client can restore into a
fresh session on another replica (`MontyShutdown` in `pydantic_monty`).
Sessions still silent after `--drain-grace` (default 30s) are dropped.
Set the pod's `terminationGracePeriodSeconds` above `--drain-grace`, and use the same `MONTY_SERVER_DUMP_KEY` on every
replica — a dump signed by one replica must verify on the one the client reconnects to.
Dumps only load into a worker of the same Monty version, so roll clients and servers together.

## Tracing

Pass `--logfire-token` (or set `LOGFIRE_TOKEN`) to export traces to [Pydantic Logfire](https://pydantic.dev/logfire).
Without a token the server behaves identically and logs to stderr only.

The server records one span per connection, and beneath it the same session, run and host-call spans every monty client
emits — a dashboard built against any monty client works here.
Policy outcomes (capacity rejections, timeouts, drain) are logged as events on the connection span, one line each naming
the limit that fired.

Traces carry caller data — the code fed to each session, call arguments, results and print output — with no flag to turn
that off.
Point the token only at a backend those callers may be exposed to.

## Security

There is no authentication and the listener speaks plain `ws://`.
Terminate TLS at an ingress or load balancer and keep the listener on a private network.
Behind a proxy you control, set `--trust-forwarded-for` so per-caller limits key on `X-Forwarded-For` rather than the
proxy's address; never set it on a directly exposed listener, where callers write the header themselves.
