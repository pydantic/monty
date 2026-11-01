# Full Monty

Full Monty runs Monty sessions on remote workers.
A host-facing `monty-server` relays each WebSocket session to a separate `monty-worker` service.
The worker runs sandboxed code and enforces execution and memory limits.
The server owns client admission, session storage, upstream credentials and graceful drain.

Python clients use `pydantic_monty.AsyncMontyWebsocket`.
Node clients use `MontyWebsocket` from `@pydantic/monty`.
The server and worker images are distributed separately from the open-source interpreter and SDKs.
Contact [Pydantic](https://pydantic.dev/contact) for access to the self-hosted images.

## Quickstart

Use matching server and worker image tags supplied with your image access.
Use an SDK version compatible with those images.
A newer source checkout does not imply that matching images have been published.

Authenticate the container client with your supplied pull credential:

```bash
docker login us-docker.pkg.dev -u _json_key --password-stdin < key.json
export MONTY_IMAGE_TAG=REPLACE_WITH_YOUR_IMAGE_TAG
export MONTY_SDK_VERSION=REPLACE_WITH_A_COMPATIBLE_SDK_VERSION
```

Save this as `compose.yaml`:

```yaml
services:
  worker:
    image: us-docker.pkg.dev/pydantic-public-registries/monty/monty-worker:${MONTY_IMAGE_TAG:?Set a matching image tag}
    read_only: true
    tmpfs:
      - /tmp
  server:
    image: us-docker.pkg.dev/pydantic-public-registries/monty/monty-server:${MONTY_IMAGE_TAG:?Set a matching image tag}
    read_only: true
    tmpfs:
      - /tmp
    environment:
      MONTY_SERVER_WORKER_URL: ws://worker:8000/
    ports:
      - '127.0.0.1:8000:8000'
    depends_on:
      - worker
```

Start both services and inspect their logs:

```bash
docker compose up -d
docker compose logs server worker
```

The server requires `MONTY_SERVER_WORKER_URL`.
It does not spawn the interpreter itself, and it does not need a dump-signing key.
The server logs its bound address to stderr.
`GET /health` reports whether the server accepts traffic; it does not establish worker availability.
The SDK journey below checks both services.

Create a client project:

```bash
mkdir monty-server-quickstart
cd monty-server-quickstart
uv init --bare
uv add "pydantic-monty==${MONTY_SDK_VERSION:?Set a compatible SDK version}"
```

Save this as `client.py`:

```python test="skip"
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

Run `uv run client.py`.
It prints `2`.
Stop the services with `docker compose down` from the directory containing `compose.yaml`.
This example has no persistent store, so sessions are ephemeral.

## Configuration

Server flags use the `MONTY_SERVER_` environment prefix.
Worker flags use `MONTY_WORKER_`.
For example, `--max-sessions` maps to `MONTY_SERVER_MAX_SESSIONS` on the server and
`MONTY_WORKER_MAX_SESSIONS` on the worker.
`MONTY_BIN` is the worker's exception for `--monty-bin`.
`LOGFIRE_TOKEN` configures tracing on either service.
Command-line flags take precedence over environment variables.

| Configure on | Settings                                                                                                           |
| ------------ | ------------------------------------------------------------------------------------------------------------------ |
| Server       | Worker URL, worker connection attempts, concurrent client sessions, per-caller cap, keepalive and session lifetime |
| Server       | Object store, database connections, idle parking, volume grants and MCP connections                                |
| Worker       | Interpreter binary, sandbox memory and execution limits, idle timeout, child reuse and worker capacity             |
| Both         | Bind address, port, drain grace and optional telemetry                                                             |

Use the help from the exact image tag you deploy:

```bash
docker compose run --rm --no-deps server --help
docker compose run --rm --no-deps worker --help
```

The images supply `--host 0.0.0.0` in their default commands.
If you replace a container's command, include that flag so another container can reach it.
Prefer environment variables for credentials because process arguments can appear in shell history and process listings.

The server and worker have independent capacity limits.
The server refuses excess connections with HTTP `503`; its per-caller limit returns `429`.
Size the worker's session and memory ceilings together with its container memory limit.
A sandbox heap limit does not bound total process RSS or relay payload memory.

## Session storage

Set `MONTY_SERVER_OBJECT_STORE_URI` to enable stored sessions.
Supported URI schemes include `s3://`, `gs://`, `az://`, `file:///` and `memory://`.
Only the server needs storage credentials.
Keep the bucket or directory writable only by trusted server processes.
Without a store, sessions are ephemeral and snapshot operations are refused.

The host receives opaque record IDs instead of interpreter state bytes.
A session dump creates a checkpoint ID; loading that ID creates an independent session.
With a shared store, another compatible server replica can restore the checkpoint.
A `memory://` store loses checkpoints when its process exits and cannot provide shared storage across replicas.
A file store needs a writable mounted directory that covers its URI path.

PostgreSQL is optional for ordinary execution and session snapshots.
Profiles require the database; volumes require both the database and object store.
Run `monty-server migrate up` before starting a server that uses these database-backed features.
Serving does not apply migrations automatically.

## Kubernetes and shutdown

The [Monty Helm chart](https://github.com/pydantic/monty-helm-chart) packages the server and worker.
Select a chart and image pair that support the settings you enable.
Both services need their own capacity and resource budgets.
The server needs writable temporary storage, including when its root filesystem is read-only.
Mount an `emptyDir` at `/tmp` and a separate persistent volume for any `file:///` store.
Temporary storage is not a checkpoint store.

Terminate TLS and enforce caller authentication at your ingress or proxy.
The standalone server does not authenticate clients.
Keep the worker endpoint private to the server tier and keep object-store credentials off workers.

Set Kubernetes `terminationGracePeriodSeconds` above each service's drain grace.
During drain, readiness fails and existing sessions receive a bounded opportunity to save state.
A stored session can hand back a checkpoint ID only after the server stores its state.
Sessions that cannot reach a safe boundary before the deadline can be disconnected.
External callbacks need their own idempotency or deduplication when an application resumes interrupted work.

## Local development relay

The source checkout includes `scripts/websocket_relay.py` for SDK transport tests.
It bridges each connection directly to a fresh `monty subprocess` and has no session store or authentication.
Run it on loopback for local development:

```bash
uv run scripts/websocket_relay.py --monty-bin /path/to/monty --max-sessions 8
```

The script prints its WebSocket URL to stdout.
`--port 0` selects an ephemeral port.
`--max-message-bytes` bounds messages in both directions and defaults to the protocol's 256 MiB ceiling.
An excess session closes with WebSocket status `1013`; an oversized message closes with status `1009`.
The relay reaps a session's subprocess when either transport direction ends.
Use the server and worker deployment above when you need persistence, server-side limits or drain.

## Tracing

Set `LOGFIRE_TOKEN` separately on the services whose telemetry you want to export.
Without a token, services keep local logging without exporting traces.
Execution telemetry can include code, callback arguments, results and printed output.
Export it only to a project authorized to receive that data.
