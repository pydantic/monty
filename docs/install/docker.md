# Install with Docker

[`monty-server`](../server.md) is the commercial way to run Monty: a WebSocket server, distributed as a container image,
that hosts the same `monty` workers the Python and JavaScript packages spawn locally.
Code that runs against a local pool runs unchanged against it; the server adds capacity limits, per-caller quotas,
health probes, graceful drain and tracing to [Pydantic Logfire](https://pydantic.dev/logfire).

The server is closed source.
Access comes with a `key.json` service-account file that authenticates image pulls; for licensing, [contact
us](https://pydantic.dev/contact).

## Pull and run

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

The image currently has only a `linux/amd64` manifest.
On an Apple Silicon Mac or another ARM64 host add `--platform=linux/amd64` to run it under emulation; without it the
pull fails with `no matching manifest for linux/arm64/v8`.

When the server is ready it prints its bound URL to stdout:

```text
ws://0.0.0.0:8000/
```

## Connect

`pydantic_monty.AsyncMontyWebsocket` connects from Python; install it with `uv add pydantic-monty`:

```python test="skip"
import asyncio

from pydantic_monty import AsyncMontyWebsocket


async def main() -> None:
    async with AsyncMontyWebsocket('ws://localhost:8000/') as pool:
        async with pool.checkout() as session:
            result = await session.feed_run('1 + 1')
            print(result)
            #> 2


asyncio.run(main())
```

The TypeScript package is subprocess-only today and cannot dial a remote server.

## Next

[Running monty-server](../server.md) covers the flags, per-session resource limits, sizing the container, Kubernetes
image pull secrets, health probes, drain and tracing.
