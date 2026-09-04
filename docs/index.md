---
title: Monty
description: "A sandboxed Python interpreter written in Rust for code written by AI. Start latency <1ms. Pause and resume. Resource limits. Available from PyPI, NPM and crates.io."
---

# Monty

<p>
  <a href="https://github.com/pydantic/monty/actions/workflows/ci.yml?query=branch%3Amain"><img src="https://github.com/pydantic/monty/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://codecov.io/gh/pydantic/monty"><img src="https://codecov.io/gh/pydantic/monty/graph/badge.svg?token=HX4RDQX5OG" alt="Coverage"></a>
  <a href="https://pypi.python.org/pypi/pydantic-monty"><img src="https://img.shields.io/pypi/v/pydantic-monty.svg" alt="PyPI"></a>
  <a href="https://www.npmjs.com/package/@pydantic/monty"><img src="https://img.shields.io/npm/v/@pydantic/monty.svg" alt="NPM"></a>
  <a href="https://crates.io/crates/monty"><img src="https://img.shields.io/crates/v/monty.svg" alt="crates.io"></a>
  <a href="https://github.com/pydantic/monty/blob/main/LICENSE"><img src="https://img.shields.io/github/license/pydantic/monty.svg?v=2" alt="license"></a>
  <a href="https://logfire.pydantic.dev/docs/join-slack/"><img src="https://img.shields.io/badge/Slack-Join%20Slack-4A154B?logo=slack" alt="Join Slack"></a>
</p>

A minimal, secure Python 3.14 interpreter written in Rust for use by AI.

Monty avoids the latency, complexity and cost of using a full container based sandbox for running LLM generated code.

## Latency

![Time to create a sandbox and run 10 REPL commands](img/startup-latency.svg)

| Sandbox                      | Cold start | Agent run, warm† | Combined‡ |
| ---------------------------- | ---------- | ---------------- | --------- |
| Monty                        | 5 ms       | 0.4 ms           | 5 ms      |
| WASI / wasmtime              | 16 ms      | 180 ms           | 200 ms    |
| Docker                       | 195 ms     | 700 ms           | 900 ms    |
| Sandboxing service (Daytona) | 1500 ms    | 400 ms           | 1900 ms   |
| Pyodide in Deno              | 2700 ms    | 35 ms            | 2700 ms   |

† 10 commands run in a REPL against a sandbox that already exists, as you might expect from a simple agent with code
mode.
Monty keeps the session, so each command is one feed; the others have no persistent interpreter, so command *n* re-runs
commands 1 to *n*.

‡ The time to create the sandbox and perform the agent run: the two columns added together.

Learn more in the [comparison to alternatives](alternatives.md).

## Why Monty

1. **Latency in microseconds, not seconds.** A sandbox plus ten REPL commands takes 5 ms against 900 ms for Docker and
    1900 ms for a sandboxing service, because the sandbox is a subprocess, a command is one message each way, and the
    session persists so nothing is re-run.
    See [start latency](#latency).
1. **Suspend and resume from bytes.** Every host call suspends the interpreter; `feed_start` returns the suspension and
    `dump()` serialises the whole interpreter, paused call stack included, to bytes you can store and `load_snapshot`
    later on another machine.
    There are no file descriptors, sockets or threads inside the sandbox, so nothing has to be reconstructed.
    See [snapshots](snapshots.md).
1. **Strict resource limits** `max_memory`, `max_duration_secs`, `max_recursion_depth` and
    `max_suspensions` are enforced by the VM itself; `'x' * 10**12` raises `MemoryError` before the allocation is
    attempted.
    See [resource limits](resource-limits.md).
1. **A package, not infrastructure.** `uv add pydantic-monty`, `npm install @pydantic/monty` or `cargo add monty-pool`:
    about 4.5 MB, no daemon, no image, no API key, and a worker baseline of about 2 MB so one machine runs hundreds.
    See [getting started](quickstart/python.md).
1. **MIT licensed, with commercial options.** The interpreter, the pool and bindings are open source.
    [`monty-server`](server.md) runs the same workers behind a WebSocket as a container image, adding OS-level isolation,
    and horizontal scaling.

## Example

Installation

=== "Python"

    ```bash
    uv add pydantic-monty
    ```

=== "TypeScript"

    ```bash
    npm install @pydantic/monty
    ```

=== "Rust"

    ```bash
    cargo add monty
    ```

The `code` string is what a model writes when asked how long a bar of chocolate could power a lightbulb.
It calls a tool it was given, does arithmetic it should not do in its head, and prints the answer:

```python
from pydantic_monty import Monty

code = """
kcal = nutrition('chocolate bar')['kcal']
hours = kcal * 4184 / (bulb_watts * 3600)
print(f'a chocolate bar could power a {bulb_watts}W bulb for {hours:.1f} hours')
"""

with Monty() as pool:
    with pool.checkout() as session:
        session.feed_run(
            code,
            inputs={'bulb_watts': 10},
            external_lookup={'nutrition': lambda food: {'kcal': 230}},
        )
        #> a chocolate bar could power a 10W bulb for 26.7 hours
```

Or in TypeScript:

```ts
import { Monty } from '@pydantic/monty'

const code = `
kcal = nutrition('chocolate bar')['kcal']
hours = kcal * 4184 / (bulb_watts * 3600)
print(f'a chocolate bar could power a {bulb_watts}W bulb for {hours:.1f} hours')
`

await using pool = await Monty.create()
await using session = await pool.checkout()
await session.feedRun(code, {
  inputs: { bulb_watts: 10 },
  externalLookup: { nutrition: (food: string) => ({ kcal: 230 }) },
})
// a chocolate bar could power a 10W bulb for 26.7 hours
```

`nutrition` ran on the host and the sandbox saw only its return value; the sandbox has no filesystem, environment or
network with which to reach anything else.
The [Python](quickstart/python.md), [JavaScript](quickstart/javascript.md) and [Rust](quickstart/rust.md) quickstarts
take it from here.
Monty can do much more than this, see [Examples](examples.md).

## Where the code comes from

LLMs are often faster, cheaper and more reliable when they write a short program that calls your tools, instead of
making a sequence of individual tool calls: [code mode](https://blog.cloudflare.com/code-mode/) from Cloudflare,
[programmatic tool calling](https://platform.claude.com/docs/en/agents-and-tools/tool-use/programmatic-tool-calling) and
[code execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp) from Anthropic,
[smolagents](https://github.com/huggingface/smolagents) from Hugging Face.
All of them need somewhere safe to run the generated code, and Monty is that place.

## Next steps

- Getting started with [Python](quickstart/python.md), [JavaScript](quickstart/javascript.md) or
    [Rust](quickstart/rust.md).
- [Commercial support](server.md): `monty-server`, the same workers behind a WebSocket as a container image.
- [Security model](security.md) for what "secure" does and does not mean here.
- [Examples](examples.md), including Code Mode in Pydantic AI.
