# Running monty-server

`monty-server` is a WebSocket server that hosts Monty sandbox workers: each connection gets its own `monty` worker
subprocess from an elastic pool.
A worker serves one session at a time and is reset or replaced between sessions, so no sandbox state crosses from one
client to another.
`pydantic_monty.AsyncMontyWebsocket` connects directly from Python; the TypeScript package (`@pydantic/monty`) is
subprocess-only today and cannot dial a remote server.
The server adds capacity limits, timeouts, per-caller quotas, health probes, graceful drain and tracing to [Pydantic
Logfire](https://pydantic.dev/logfire). The `monty` worker provides the sandboxing.

The server is closed-source and distributed as a container image.
For access and licensing, [contact us](https://pydantic.dev/contact).

## Why Monty over WebSocket

Running Monty on a remote server provides:

- **Security**: escaping the sandbox gets you the machine running Monty, not the machine running the agent or
  application code. That machine is an empty container.
- **Centralized monitoring, observability, and scaling**: one horizontally scalable service for all Monty code
  execution, instead of every service running its own worker pool.
- **Density**: Monty workers have a small baseline footprint (as little as 2MB), plus additional memory for limits and
  optional type checking, so a single machine can run hundreds.
- **Same behavior as local Monty**: the wire protocol carries host callbacks, name lookups, async futures and mounted
  client directories, so code that runs against a local pool runs unchanged against the server.
- **Future full sandbox option**: a VM running CPython for code that needs dependencies, bash or a real filesystem,
  exposed through the same interface.

## Quickstart

The image is private: commercial access comes with a `key.json` service-account file that authenticates pulls (the same
mechanism as self-hosted Logfire images, which live on the same registry host):

```bash
docker login us-docker.pkg.dev -u _json_key --password-stdin < key.json
```

The image refuses to start without a dump-signing key of at least 16 bytes:

```bash
docker run --rm \
  -e MONTY_SERVER_DUMP_KEY="$(openssl rand -hex 16)" \
  -p 8000:8000 \
  us-docker.pkg.dev/pydantic-public-registries/monty/monty-server:latest
```

The image currently only has a `linux/amd64` manifest. On an Apple Silicon Mac or another ARM64 host, run it through
Docker's amd64 emulation by adding `--platform=linux/amd64`:

```bash
docker run --rm \
  --platform=linux/amd64 \
  -e MONTY_SERVER_DUMP_KEY="$(openssl rand -hex 16)" \
  -p 8000:8000 \
  us-docker.pkg.dev/pydantic-public-registries/monty/monty-server:latest
```

Native ARM64 images are planned. Until they are available, omitting `--platform=linux/amd64` on an ARM64 host fails
with `no matching manifest for linux/arm64/v8`.

When the server is ready, it prints its bound URL to stdout:

```text
ws://0.0.0.0:8000/
```

In another terminal, create a Python client project and add `pydantic-monty`:

```bash
mkdir monty-server-quickstart
cd monty-server-quickstart
uv init --bare
uv add pydantic-monty
```

Create `client.py`:

```python
import asyncio

from pydantic_monty import AsyncMontyWebsocket


async def main() -> None:
    async with AsyncMontyWebsocket('ws://localhost:8000/') as pool:
        async with pool.checkout() as session:
            result = await session.feed_run('1 + 1')
            print(result)


if __name__ == '__main__':
    asyncio.run(main())
```

Run the client:

```bash
uv run client.py
```

It should print:

```text
2
```

`GET /` on the server URL returns a short info page.

The image bundles the matching `monty` worker binary and runs as non-root uid 65532 on a `scratch` base with no shell or
package manager. The bound URL is the only line written to stdout. All logging goes to stderr.

### Kubernetes

On Kubernetes, `key.json` becomes an image pull secret referenced from the pod spec:

```bash
kubectl create secret docker-registry monty-image-key \
  --docker-server=us-docker.pkg.dev \
  --docker-username=_json_key \
  --docker-password="$(cat key.json)"
```

## Configuration

Every configuration flag has an environment variable: the flag name in upper snake case with a `MONTY_SERVER_` prefix
(`--max-sessions` → `MONTY_SERVER_MAX_SESSIONS`), except `--monty-bin` and `--logfire-token`, which use `MONTY_BIN` and
`LOGFIRE_TOKEN` respectively.
A flag on the command line wins over its variable.

The server binary defaults to `--host 127.0.0.1`, but the image's default `CMD` supplies `--host 0.0.0.0` so a published
Docker port is reachable. Arguments after the image name in `docker run <image> ...` replace that `CMD` wholesale, so
include `--host 0.0.0.0` when passing any flags. Hostnames are accepted, but `--host localhost` resolves to the
container's loopback interface and is not reachable through `-p 8000:8000`.

| Flag                                | Meaning                                                                  | Default                                    |
| ----------------------------------- | ------------------------------------------------------------------------ | ------------------------------------------ |
| `--host <address>`                  | interface to bind                                                        | image: `0.0.0.0`; binary: `127.0.0.1`      |
| `--port <port>`                     | port to bind; 0 selects an ephemeral port                                | 8000                                       |
| `--monty-bin <path>`                | worker binary                                                            | image: `/usr/local/bin/monty`               |
| `--max-sessions <n>`                | concurrent sessions across the server                                    | 64                                         |
| `--max-sessions-per-client <n>`     | concurrent sessions per caller; 0 disables                              | 10                                         |
| `--idle-timeout <seconds>`          | maximum gap between requests; 0 disables                                 | 60                                         |
| `--keepalive <seconds>`             | WebSocket ping interval for detecting vanished clients; 0 disables       | 5                                          |
| `--session-timeout <seconds>`       | maximum total session lifetime; 0 disables                               | 3600                                       |
| `--turn-timeout <seconds>`          | wall-clock cap on one request; 0 disables                                | 300                                        |
| `--drain-grace <seconds>`           | time after SIGTERM for existing sessions to collect a dump               | 30                                         |
| `--max-memory-mib <MiB>`            | per-session memory ceiling; 0 disables                                   | 64                                         |
| `--max-duration <seconds>`          | cumulative sandbox execution time per session; 0 disables                | 60                                         |
| `--max-recursion-depth <n>`         | per-session call-stack ceiling; cannot be disabled                       | 1000                                       |
| `--trust-forwarded-for`             | use the last `X-Forwarded-For` entry as the caller identity               | off                                        |
| `--dump-key <key>`                  | required key of at least 16 bytes for signing session dumps              | none (required)                            |
| `--logfire-token <token>`           | export traces to Logfire                                                 | off                                        |

When the global session limit is full, a new WebSocket upgrade gets `503 Service Unavailable`; exceeding the
per-client limit gets `429 Too Many Requests`.

Run the image's `--help` for the authoritative list for the version you pulled:

```bash
docker run --rm \
  --platform=linux/amd64 \
  us-docker.pkg.dev/pydantic-public-registries/monty/monty-server:latest \
  --help
```

Prefer environment variables for container configuration so the image's `CMD` remains intact, especially for
`--dump-key` and `--logfire-token`: a command line is world-readable via `ps` and commonly lands in shell history.

### Resource limits

The server's three sandbox resource limits are ceilings. If a client omits a limit, the server limit applies. A higher
value is clamped to the server limit. A lower value is accepted and becomes the effective ceiling. Duration and memory
limits can be disabled, but recursion depth is always bounded.

The server flags and Python client keys use different names and, for memory, different units:

| Server flag                       | `pool.checkout(limits=...)` key | Unit    |
| --------------------------------- | ------------------------------- | ------- |
| `--max-duration <seconds>`        | `max_duration_secs`             | seconds |
| `--max-memory-mib <MiB>`          | `max_memory`                    | bytes   |
| `--max-recursion-depth <n>`       | `max_recursion_depth`           | count   |

For example, this asks for 30 seconds, 32 MiB and a recursion depth of 500; the server may lower any value to its own
ceiling:

```python
import asyncio

from pydantic_monty import AsyncMontyWebsocket


async def main() -> None:
    async with AsyncMontyWebsocket('ws://localhost:8000/') as pool:
        async with pool.checkout(
            limits={
                'max_duration_secs': 30,
                'max_memory': 32 * 1024 * 1024,
                'max_recursion_depth': 500,
            },
        ) as session:
            result = await session.feed_run('1 + 1')
            print(result)


if __name__ == '__main__':
    asyncio.run(main())
```

Save this as `limits_client.py` and run `uv run limits_client.py`; with the default server configuration, it prints
`2`.

`--max-duration` counts cumulative interpreter execution across the session and excludes time suspended waiting for
the client. `--turn-timeout` measures wall-clock time for one complete request, including time waiting for a client
callback. Keep the server's turn timeout above the clients' `request_timeout` so the client watchdog can report a more
specific failure first.

A keepalive ping that goes unanswered for one further `--keepalive` interval ends the session. Keep that interval below
half of `--idle-timeout` if keepalive should detect a vanished client first.

The worker enforces the effective limits. Memory and duration errors include the effective byte or time ceiling in
their messages.

## Sizing the container

Workers are created on demand for active connections and exit when their sessions close. `--max-sessions` is a capacity
limit, not a number of preallocated workers.

`--max-memory-mib` limits per-session live bytes requested through the worker's global allocator. It does not cap total
process memory or RSS. Thread stacks, the mapped binary image, direct `mmap`, allocator overhead and fragmentation are
not included. The allocator's hard ceiling also includes the worker baseline and 4 MiB of headroom, or 32 MiB with type
checking. Estimate the peak allocator budget as:

```
peak worker allocator budget ≈ --max-sessions × (--max-memory-mib + baseline + headroom)
```

At full utilization, the default session limits alone allow 4 GiB of allocator-backed live bytes. Container RSS will be
higher. Account for untracked worker memory and server overhead, and use an OS or cgroup memory limit to enforce a hard
container bound. Lower `--max-sessions` or `--max-memory-mib` to fit your instance.

## Health probes and drain

While the server is accepting traffic, `GET /health` returns an empty 200 response and `GET /` returns a 200 info page.
Use `/health` for readiness and `/` for liveness.

On SIGTERM, the server stops listening immediately, so new HTTP and WebSocket connections are refused rather than
receiving a 503 response. Existing WebSocket sessions remain connected while the server drains. Each existing session's
next request raises `pydantic_monty.MontyShutdown`; that protocol request did not run and can be resent after restoration.
Its `dump` contains the signed session state when state exists and dumping succeeds, or `None` otherwise. Check that the
dump is not `None`, then restore an idle dump on a fresh session with `await session.load_session(exc.dump)` before
resending the request; a dump captured while a feed is suspended instead uses
`await session.load_snapshot(exc.dump, ...)`.

If the interrupted request was answering an external function or `os` callback, the host already ran that callback and
the restored snapshot re-announces it. Make such callbacks idempotent or deduplicate them before restoring across a
shutdown.

Sessions that remain silent through `--drain-grace` (default 30s) are dropped without a dump.
Set the pod's `terminationGracePeriodSeconds` above `--drain-grace`. Use the same `MONTY_SERVER_DUMP_KEY` on every
replica so the client can restore a dump after reconnecting to a different one.
Dumps only load into a worker of the same Monty version, so roll clients and servers together.

## Tracing

Pass `--logfire-token` (or set `LOGFIRE_TOKEN`) to export traces to [Pydantic Logfire](https://pydantic.dev/logfire).
Without a token, execution is unchanged. Traces and logs are written to stderr but not exported.

The server records one span per connection. Beneath it are the same session, run and host-call spans emitted by other
Monty clients, so existing Monty dashboards work with the server.
Policy outcomes (capacity rejections, timeouts, drain) are logged as events on the connection span, one line each naming
the limit that fired.

Traces carry WebSocket handshake request headers, the code fed to each session, call arguments, results and print
output. There is no option to disable collection of these fields.
Only export to a Logfire project that is authorized to receive this caller data.

## Security

There is no authentication and the listener speaks plain `ws://`.
Terminate TLS at an ingress or load balancer and keep the listener on a private network.
By default, `--max-sessions-per-client` identifies a caller by the direct peer IP. A proxy or NAT can therefore make
many callers share one quota. Behind a proxy you control, set `--trust-forwarded-for` to identify callers by the last
`X-Forwarded-For` entry instead. Configure that proxy to sanitize the header, and never enable this flag on a directly
exposed listener, where callers can forge a fresh identity per connection and bypass the quota.
