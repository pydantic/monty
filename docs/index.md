---
title: Monty
description: "A secure Python sandbox written in Rust for code written by AI. Start latency <1ms. Pause and resume. Resource limits. Available from PyPI, NPM and crates.io."
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

!!! success "🎉 Monty v1.0.0 is released"

    Monty has reached its first stable release, and we now consider it ready for production use
    (although it is provided "as is" without any warranty, as per the [license](https://github.com/pydantic/monty/blob/main/LICENSE)).
    See the [v1.0.0 release notes on GitHub](https://github.com/pydantic/monty/releases/tag/v1.0.0) for what's changed.

A minimal, secure Python sandbox written in Rust for code written by AI.

Monty avoids the latency, complexity and cost of a container based sandbox for running LLM generated code.
It comes in two forms: **OSS Monty**, the MIT licensed Python 3.14 sandbox you install as a package, and
[**Full Monty**](server.md), the commercial server that runs the same sandbox behind a WebSocket as a service.

## Latency

![Time to get a new sandbox and run 10 REPL commands](img/startup-latency.svg)

| Sandbox                | New sandbox¹ | Agent run² | Combined³ | Execution env⁴ |
| ---------------------- | ------------ | ---------- | --------- | -------------- |
| OSS Monty              | 0.80 ms      | 0.40 ms    | 1.20 ms   | local          |
| Full Monty (WebSocket) | 1.70 ms      | 5.30 ms    | 7.00 ms   | remote         |
| WASI / wasmtime        | 16 ms        | 180 ms     | 200 ms    | local          |
| local Docker           | 195 ms       | 700 ms     | 900 ms    | local          |
| Sandboxing service     | 1500 ms      | 400 ms     | 1900 ms   | remote         |
| Pyodide in Deno        | 2700 ms      | 35 ms      | 2700 ms   | local          |

1. **New sandbox**: the time to get a fresh sandbox and run `1 + 1` in it.
    For OSS Monty and Full Monty that is a checkout from a pool the application already created.
    The others have no pool, so each new sandbox starts from nothing.
1. **Agent run**: 10 commands run in a REPL against a sandbox that already exists, as you might expect from a simple
    agent with code mode.
    OSS Monty and Full Monty keep the session, so each command is one feed; the others have no persistent interpreter,
    so command *n* re-runs commands 1 to *n*.
1. **Combined**: the time to create the sandbox and perform the agent run: the two columns added together.
1. **Execution env**: OSS Monty, WASI, local Docker and Pyodide run the code on the same machine as the application
    calling them.
    Full Monty and sandboxing services run it remotely, which reduces the blast radius of an escape and lets the
    sandboxes scale independently of the hosts calling them.

Learn more in the [comparison to alternatives](alternatives.md).

!!! tip "Commercial support"

    If you're interested in running Monty in the most secure and scalable setup, please see
    [Full Monty](server.md).
    If you're interested in being a design partner for Monty development in any deployment setup, please
    [get in touch](https://pydantic.dev/contact).

## Why Monty

1. **Latency in milliseconds, not seconds.** A new sandbox plus ten REPL commands takes 1.2 ms vs. 1900 ms for
    a sandboxing service, because a sandbox is a checkout from a pool of worker subprocesses, a command is one
    message each way, and the session persists so nothing is re-run.
    See [start latency](#latency).
1. **Simple to deploy at massive scale.** Because you're not provisioning a new VM or container for every sandbox
    you can run thousands of workers with minimal cost and complexity.
1. **Suspend and resume from bytes.** Monty lets you dump the whole sandbox state to bytes at an external function call
    or at the end of a repl snippet. This makes long external function calls and human-in-the-loop not only possible
    but very cheap. It also makes extremely long running REPL sessions easy to implement.
    See [snapshots](snapshots.md).
1. **Strict resource limits** maximum memory and execution time are enforced by the VM itself
    so `'x' * 10**12` raises `MemoryError` before the allocation is attempted.
    See [resource limits](resource-limits.md).
1. **Local package for development, commercial option for scale.** OSS Monty provides packages for Python, JS and Rust,
    making it trivial to get started with Monty. For greater security guards and larger scale deployments,
    [Full Monty](server.md) runs the same workers behind a WebSocket as a container image, adding OS-level isolation
    and horizontal scaling.

## Example

Installation

=== "Python"

    ```bash
    uv add pydantic-monty
    ```

    See [getting started with Python](quickstart/python.md).

=== "TypeScript"

    ```bash
    npm install @pydantic/monty
    ```

    See [getting started with JavaScript](quickstart/javascript.md).

=== "Rust"

    ```bash
    cargo add monty-pool
    ```

    See [getting started with Rust](quickstart/rust.md).

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
- [Commercial support](server.md): Full Monty, the same workers behind a WebSocket as a container image.
- [Security model](security.md) for what "secure" does and does not mean here.
- [Examples](examples.md), including Code Mode in Pydantic AI.
- [Limitations](limitations/index.md): the Python subset, and every known divergence from CPython.
