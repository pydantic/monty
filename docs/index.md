# Monty

A minimal, secure Python interpreter written in Rust for use by AI.

Monty runs Python written by an LLM without the cost, latency and complexity of a container sandbox.
It does not embed CPython: it parses Python with [Ruff](https://github.com/astral-sh/ruff)'s parser and executes it on
its own bytecode VM, with no FFI and no C dependencies.
That is what makes it small enough to install from PyPI or npm, fast enough to start per request, and portable enough to
run anywhere Rust runs, including WebAssembly.

The sandbox has no ambient access to the machine it runs on.
Filesystem, environment variables and network are reachable only through [host functions](host-functions.md) and
[mounts](filesystem.md) that you hand it explicitly.

## Why this exists

LLMs are often faster, cheaper and more reliable when they write a short program that calls your tools, instead of
making a sequence of individual tool calls.
That idea goes by several names:

- [Code mode](https://blog.cloudflare.com/code-mode/) from Cloudflare
- [Programmatic tool calling](https://platform.claude.com/docs/en/agents-and-tools/tool-use/programmatic-tool-calling)
  from Anthropic
- [Code execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp) from Anthropic
- [smolagents](https://github.com/huggingface/smolagents) from Hugging Face

All of them need somewhere safe to run the generated code.
Monty is that place, without a container or a sandboxing service in the loop.

## What Monty can do

- **Run a useful subset of Python** — functions, closures, decorators, classes, dataclasses, comprehensions,
  `try`/`except`, f-strings, `async`/`await`, and the most commonly-used stdlib modules.
  See [the Python subset](python-subset.md).
- **Block host access by default** — an unmounted sandbox cannot read a file, read an environment variable, open a
  socket or spawn a process.
  See the [security model](security.md).
- **Call functions you provide** — the sandbox suspends, your code runs the real function on the host, execution resumes
  with the result.
  Sync or async.
  See [host functions](host-functions.md).
- **Expose objects and classes you choose** — per-attribute and per-method allow-lists, with method calls and
  construction routed back to the host.
  See [host objects](host-objects.md).
- **Type check before running** — Monty bundles [ty](https://docs.astral.sh/ty/) and a trimmed typeshed of Monty's
  runtime surface, so unsupported APIs generally fail up front rather than halfway through.
  See [type checking](type-checking.md).
- **Snapshot and resume** — a paused interpreter serializes to bytes you can store in a file or a database and resume
  later, in another process or on another machine.
  See [snapshots](snapshots.md).
- **Bound resource use** — limits on heap memory, cumulative execution time, recursion depth and GC interval.
  See [resource limits](resource-limits.md).
- **Contain crashes** — the Python package and the native `@pydantic/monty` binding run every session in a worker
  subprocess, so even a stack-overflow abort triggered by adversarial code kills only the worker.
  The WebAssembly build has no subprocess to use; see [the security model](security.md#in-process-execution).
- **Be called from Rust, Python or JavaScript** — and in the browser, via a WebAssembly build.

## What Monty cannot do

- **Most of the standard library.** Only a [subset of standard library modules](python-subset.md) is available, and each
  module covers only part of its CPython surface.
- **Third-party packages.** There is no `sys.path` and no site-packages inside the sandbox; supporting PyPI packages is
  not a goal.
- **Class inheritance.** `class Foo(Bar):` is rejected at parse time, and so are method decorators like `@classmethod`,
  `@staticmethod` and `@property`; `super()` raises `NameError`.
  Simple classes without a base class do work.
- **Generators, `match` statements, `del`, `async with`, `async for`, exception groups, PEP 695 `type` aliases, complex
  numbers and t-strings.** All are rejected at parse time.
- **User-defined exception classes.** The built-in exception types are a fixed set.

The exhaustive, per-feature list of how Monty diverges from CPython lives in
[`limitations/`](https://github.com/pydantic/monty/tree/main/limitations) in the repository.
[The Python subset](python-subset.md) explains how to read it.

## When to reach for Monty

Monty is a good fit when the code is written by a model, is short-lived, and mostly glues together tools you already
own: fetch these three things, join them, filter, do some arithmetic, return the answer.

It is a poor fit for anything that needs the real Python ecosystem — notebooks, data science, user-supplied scripts that
import `pandas`.
For those, a container or a sandboxing service is still the right tool.
The [comparison table in the README](https://github.com/pydantic/monty#alternatives) walks through the alternatives and
where each one wins.

## Next steps

- [Installation](install.md) for Python, JavaScript and Rust.
- QuickStart for [Python](quickstart/python.md), [JavaScript](quickstart/javascript.md) or [Rust](quickstart/rust.md).
- [Security model](security.md) for what "secure" does and does not mean here.

Monty powers [Code Mode](https://pydantic.dev/docs/ai/harness/code-mode/) in
[Pydantic AI](https://github.com/pydantic/pydantic-ai).

## Part of the Pydantic Stack

- [Pydantic AI](https://pydantic.dev/pydantic-ai) — type-safe agent framework
- [Pydantic Logfire](https://pydantic.dev/logfire) — AI-first, full-stack observability
- [Logfire AI Gateway](https://pydantic.dev/ai-gateway) — unified LLM proxy
