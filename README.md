<div align="center">
  <h1>Monty</h1>
</div>
<div align="center">
  <h3>A sandboxed Python interpreter, written in Rust, for code written by AI.</h3>
</div>
<div align="center">
  <a href="https://github.com/pydantic/monty/actions/workflows/ci.yml?query=branch%3Amain"><img src="https://github.com/pydantic/monty/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://codspeed.io/pydantic/monty?utm_source=badge"><img src="https://img.shields.io/badge/CodSpeed-Performance%20Tracked-blue?logo=data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMTYiIGhlaWdodD0iMTYiIHZpZXdCb3g9IjAgMCAxNiAxNiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj48cGF0aCBkPSJNOCAwTDAgOEw4IDE2TDE2IDhMOCAwWiIgZmlsbD0id2hpdGUiLz48L3N2Zz4=" alt="Codspeed"></a>
  <a href="https://codecov.io/gh/pydantic/monty"><img src="https://codecov.io/gh/pydantic/monty/graph/badge.svg?token=HX4RDQX5OG" alt="Coverage"></a>
  <a href="https://pypi.python.org/pypi/pydantic-monty"><img src="https://img.shields.io/pypi/v/pydantic-monty.svg" alt="PyPI"></a>
  <a href="https://www.npmjs.com/package/@pydantic/monty"><img src="https://img.shields.io/npm/v/@pydantic/monty.svg" alt="NPM"></a>
  <a href="https://crates.io/crates/monty"><img src="https://img.shields.io/crates/v/monty.svg" alt="crates.io"></a>
  <a href="https://github.com/pydantic/monty/blob/main/LICENSE"><img src="https://img.shields.io/github/license/pydantic/monty.svg?v=2" alt="license"></a>
  <a href="https://logfire.pydantic.dev/docs/join-slack/"><img src="https://img.shields.io/badge/Slack-Join%20Slack-4A154B?logo=slack" alt="Join Slack" /></a>
</div>

---

> [!NOTE]
> **Hack Monty Round 3 is live** - the last round before Monty V1. See [pydantic.dev/monty](https://pydantic.dev/monty) for details.

Monty runs Python written by a model with no container, VM or sandboxing service in the loop.
Creating a sandbox and running ten commands in it takes 5 ms (Docker: 900 ms, a sandboxing service: 1900 ms), a paused
interpreter serialises to bytes you can resume later, and memory, time and recursion limits are enforced by the VM
itself.
Filesystem, environment variables and network do not exist inside the sandbox: it reaches the host only through the
functions and mounts you pass in.

**Documentation: [pydantic.dev/docs/monty](https://pydantic.dev/docs/monty/)**

## Install

```bash
uv add pydantic-monty        # Python
npm install @pydantic/monty  # JavaScript / TypeScript
cargo add monty              # Rust
```

The commercial [`monty-server`](https://pydantic.dev/docs/monty/commercial-support/server/) runs the same workers as a container
image.

## Example

The `code` string is what a model writes when asked how long a bar of chocolate could power a lightbulb:

```python
from pydantic_monty import Monty

code = """
kcal = nutrition('chocolate bar')['kcal']
hours = kcal * 4184 / (bulb_watts * 3600)
print(f'a chocolate bar powers a {bulb_watts} W bulb for {hours:.1f} hours')
"""

with Monty() as pool:
    with pool.checkout() as session:
        session.feed_run(
            code,
            inputs={'bulb_watts': 10},
            external_lookup={'nutrition': lambda food: {'kcal': 230}},
        )
        #> a chocolate bar powers a 10 W bulb for 26.7 hours
```

`nutrition` ran on the host and the sandbox saw only its return value.

## Documentation

- [Introduction](https://pydantic.dev/docs/monty/) with the latency measurements
- [Comparison to alternatives](https://pydantic.dev/docs/monty/reference/alternatives/): Docker, Pyodide, WASI,
  sandboxing services
- Getting started with [Python](https://pydantic.dev/docs/monty/quickstart/python/),
  [JavaScript](https://pydantic.dev/docs/monty/quickstart/javascript/) or
  [Rust](https://pydantic.dev/docs/monty/quickstart/rust/)
- [Security model](https://pydantic.dev/docs/monty/concepts/security/), [resource
  limits](https://pydantic.dev/docs/monty/concepts/resource-limits/),
  [snapshots](https://pydantic.dev/docs/monty/concepts/snapshots/), [the Python
  subset](https://pydantic.dev/docs/monty/reference/python-subset/)
- [`limitations/`](./limitations): the exhaustive record of how Monty diverges from CPython
- [`docs/`](./docs): the source of the documentation site

Monty runs [Code Mode](https://pydantic.dev/docs/ai/harness/code-mode/) in [Pydantic
AI](https://github.com/pydantic/pydantic-ai).
Community bindings: [gomonty](https://github.com/ewhauser/gomonty/) (Go) and
[dart_monty](https://github.com/runyaga/dart_monty) (Dart / Flutter).

## Part of the Pydantic Stack

The Pydantic Stack is everything you need to ship production-grade AI agents:

- [Pydantic AI](https://pydantic.dev/pydantic-ai?utm_source=github&utm_medium=readme&utm_campaign=monty) - Type-safe
  agent framework
- [Pydantic Logfire](https://pydantic.dev/logfire?utm_source=github&utm_medium=readme&utm_campaign=monty) - AI-first,
  full-stack observability
- [Logfire AI Gateway](https://pydantic.dev/ai-gateway?utm_source=github&utm_medium=readme&utm_campaign=monty) - Unified
  LLM proxy
