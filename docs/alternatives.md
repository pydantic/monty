# Comparison to Alternatives

There are generally two responses when you show people Monty:

1. This solves so many problems, I want it.
1. Why not X?

Oddly often these responses are combined: people have not found an alternative that works for them, but are incredulous
that there is really no better option than writing a Python implementation from scratch.

This page runs through the most obvious alternatives and why they were not right for what we wanted: somewhere to run
code written by a model, per request, with nothing else in the loop.
All of these technologies are impressive and widely used.
Most were not conceived as an LLM sandbox, which is why they are not necessarily great at being one.

![Time to create a sandbox and run 10 REPL commands](img/startup-latency.svg)

The chart is the time to create a sandbox and then run ten REPL commands in it; both halves are measured below.

| Tech               | Language completeness | Security     | Start latency                | FOSS       | Setup complexity | File mounting  | Snapshotting |
| ------------------ | --------------------- | ------------ | ---------------------------- | ---------- | ---------------- | -------------- | ------------ |
| Monty              | partial               | strict       | 0.08 ms warm pool, 5 ms cold | free / OSS | easy             | easy           | easy         |
| Docker             | full                  | good         | 195 ms                       | free / OSS | intermediate     | easy           | intermediate |
| Pyodide            | full                  | poor         | 2700 ms                      | free / OSS | intermediate     | easy           | hard         |
| starlark-rust      | very limited          | good         | 1.3 ms                       | free / OSS | easy             | not available? | impossible?  |
| WASI / wasmtime    | partial, almost full  | strict       | 16 ms                        | free / OSS | intermediate     | easy           | intermediate |
| sandboxing service | full                  | strict       | 1500 ms                      | not free   | intermediate     | hard           | intermediate |
| YOLO Python        | full                  | non-existent | 0.1 ms / 30 ms               | free / OSS | easy             | easy / scary   | hard         |

Start latency is the time from requesting a sandbox to receiving the result of `1 + 1`.
The agent run below is ten REPL commands against a sandbox that already exists.
Both come from
[`scripts/startup_performance.py`](https://github.com/pydantic/monty/blob/main/scripts/startup_performance.py); the
chart adds them.

### Agent run

Start latency measures one execution.
An agent in code mode sends several blocks to one environment, each building on the last, so the same script also times
ten REPL feeds against a sandbox that already exists, and the chart above adds the two:

| Sandbox                                      | Cold start | Agent run, warm† | Combined |
| -------------------------------------------- | ---------- | ---------------- | -------- |
| Monty, warm pool                             | 0.08 ms    | 0.4 ms           | 0.5 ms   |
| Monty, cold start                            | 5 ms       | 0.4 ms           | 5 ms     |
| WASI / wasmtime, precompiled CPython         | 16 ms      | 180 ms           | 200 ms   |
| Docker, running container, `docker exec`     | 195 ms     | 700 ms           | 900 ms   |
| Sandboxing service, existing Daytona sandbox | 1500 ms    | 400 ms           | 1900 ms  |
| Pyodide, running Deno sandbox                | 2700 ms    | 35 ms            | 2700 ms  |

The two Monty rows differ only in whether a worker already exists in the pool; the chart uses the cold one.

† 10 commands run in a REPL, as you might expect from a simple agent with code mode.
Monty keeps the session, so each command is one `feed_run`.
None of the others has a persistent interpreter to feed: `python.wasm` is a WASI command module whose `_start` runs
once, a container or a service runs one program per request, and the Pyodide sandbox evaluates each call in fresh
globals.
For those, command *n* re-runs commands 1 to *n*, the cheapest strategy that gives the same result, so the cost is ten
interpreter starts plus the replayed work.
The commands themselves are in `AGENT_BLOCKS` in the script: a list of orders, a function, comprehensions, `json`, and
an f-string report; every setup must print the same report.

### How each setup was measured

Every row was measured on 2026-09-03 on an Apple M3 Max (96 GB, macOS 26.5.2) in London, from CPython 3.14.7, with a
single sample per cold start unless stated.
Numbers are rounded to two significant figures.

- **Monty**: `pydantic-monty` 0.0.21 with a release build of the `monty` worker binary, driven through `Monty()` /
    `pool.checkout()` / `session.feed_run()`, the package's only execution API.
    Cold start creates the pool, which spawns the worker subprocess, completes the protocol handshake, checks out a
    session and runs `1 + 1`; the median of 7 runs is 4.5 ms.
    Warm pool is the median of 20 `checkout()` + `feed_run()` round trips against a pool whose worker already exists.
    The agent run is ten `feed_run` calls on one checkout, so state persists and nothing is replayed.
- **WASI / wasmtime**: the [CPython 3.14.7 WASI build](https://github.com/brettcannon/cpython-wasi-build) (`python.wasm`
    plus its `lib/` directory, preopened as `/` with `PYTHONHOME=/`) run in-process through the
    [`wasmtime`](https://pypi.org/project/wasmtime/) 48.0.0 Python package.
    The module is compiled once to a `.cwasm` file ahead of time, as a deployment would; the timed cold start deserialises
    it (about 1.5 ms), instantiates, and runs `python -c 'print(1 + 1)'`, which is dominated by CPython's own startup
    inside the module.
    Compiling from wasmtime's cache instead costs about 95 ms, and from scratch about 340 ms.
    The agent run deserialises once and creates one `Store` per command, replaying the earlier commands; deserialising a
    new module while the previous store is still alive would add about 200 ms of page faults per command.
- **Docker**: Docker Desktop 29.6.2 with the `python:3.14-alpine` image already pulled.
    Cold start is `docker run --rm python:3.14-alpine python -c 'print(1 + 1)'`.
    The agent run keeps one container alive (`docker run -d --rm python:3.14-alpine sleep infinity`) and executes each
    replayed program with `docker exec <container> python -c ...`, so it pays for `docker exec` and a CPython start per
    command but not for a container start.
- **Sandboxing service**: [Daytona](https://daytona.io) through the `daytona` 0.207.0 SDK, sandboxes in Daytona's EU
    region, called from London.
    Cold start is `Daytona().create()` followed by `sandbox.process.code_run("print(1 + 1)")`.
    The agent run creates a sandbox, warms it with one call, then makes ten `code_run` calls with the replayed programs,
    so each command is one HTTPS round trip plus a CPython start on the sandbox; the sandbox is deleted afterwards.
    Daytona advertises sub-90 ms sandbox creation; the 1.5 s measured here includes the network round trips from London.
- **Pyodide**: [`mcp-run-python`](https://pypi.org/project/mcp-run-python/) 0.0.22, which starts a Deno 2.5.5 process
    running Pyodide 0.28.2 and exposes it as an MCP server over stdio.
    Cold start is `code_sandbox()`, which spawns Deno and loads Pyodide, followed by one `eval`; installing a package such
    as `numpy` at start adds about 200 ms more.
    The agent run reuses a started sandbox and makes ten `eval` calls with the replayed programs; each call is an MCP
    round trip into the already-loaded Pyodide, which keeps no globals between calls.
- **starlark-rust**: [`starlark-pyo3`](https://pypi.org/project/starlark-pyo3/) 2026.1.1, in-process; the 1.3 ms is the
    first `parse` + `eval` after import, later evaluations take about 0.01 ms.
    It has no agent-run row because the commands are Python, not Starlark.
- **YOLO Python**: `eval("1 + 1")` in the measuring process (about 0.1 ms) and `python -c 'print(1 + 1)'` as a
    subprocess (about 30 ms).
    Replaying the agent run through ten subprocesses takes about 180 ms; ten `exec` calls into one namespace take 0.3 ms.

## Monty

- **Language completeness**: no class inheritance, limited stdlib, no third-party libraries.
    See [the Python subset](limitations/index.md).
- **Security**: explicitly controlled filesystem, network and environment access; strict limits on execution time and
    memory usage.
    See the [security model](security.md).
- **Start latency**: a warm checkout is one message to a worker that already exists; a cold start spawns the worker.
- **Setup complexity**: `pip install pydantic-monty` or `npm install @pydantic/monty`, about 4.5 MB download.
- **File mounting**: strictly controlled, see [filesystem access](filesystem.md).
- **Snapshotting**: `feed_start()` and `dump()` pause, resume and fork execution.
    See [snapshots](snapshots.md).

## Docker

- **Language completeness**: full CPython with any library.
- **Security**: process and filesystem isolation, network policies, but container escapes exist; memory limitation is
    possible.
- **Start latency**: container startup overhead, 195 ms measured.
- **Setup complexity**: requires the Docker daemon, container images and orchestration; `python:3.14-alpine` is 50 MB
    and Docker cannot be installed from PyPI.
- **File mounting**: volume mounts work well.
- **Snapshotting**: possible with durable execution solutions like Temporal, or by snapshotting a container and saving
    it as an image.

## Pyodide

- **Language completeness**: full CPython compiled to WASM, almost all libraries available.
- **Security**: relies on the browser/WASM sandbox and is not designed for server-side isolation; Python code can run
    arbitrary code in the JS runtime; only Deno allows isolation, and memory limits are hard or impossible to enforce with
    Deno.
- **Start latency**: loading the WASM runtime is slow, 2700 ms cold start measured.
- **Setup complexity**: load the WASM runtime and handle async initialisation; the Pyodide npm package is about 12 MB
    and Deno about 50 MB, so Pyodide cannot be used with PyPI packages alone.
- **File mounting**: virtual filesystem via browser APIs.
- **Snapshotting**: presumably possible with durable execution solutions like Temporal, but hard.

## starlark-rust

See [starlark-rust](https://github.com/facebook/starlark-rust).

- **Language completeness**: a configuration language, not Python; no classes, exceptions or async.
- **Security**: deterministic and hermetic by design.
- **Start latency**: runs embedded in the process; 1.3 ms for the first evaluation, around 0.01 ms after that.
- **Setup complexity**: usable from Python via [starlark-pyo3](https://github.com/inducer/starlark-pyo3).
- **File mounting**: no file handling by design, as far as we know.
- **Snapshotting**: impossible, as far as we know.

## WASI / wasmtime

CPython compiled to WebAssembly (WASI), run by [wasmtime](https://wasmtime.dev/).

- **Language completeness**: full CPython; pure-Python packages work from a mounted directory, packages with C
    extensions need their own WASI build.
    In the WASI build `socket.socket()` and `subprocess.run()` raise `OSError`, `threading.Thread.start()` raises
    `RuntimeError`, and `ctypes` does not import.
- **Security**: the WebAssembly sandbox plus WASI's capability model; the guest sees only the directories and
    environment variables you preopen.
- **Start latency**: 16 ms with the module precompiled to a `.cwasm` file ahead of time, as a deployment would; about 95
    ms when wasmtime compiles from its cache and about 340 ms compiling from scratch.
    Measured in-process through the [`wasmtime`](https://pypi.org/project/wasmtime/) Python package with the [CPython
    3.14.7 WASI build](https://github.com/brettcannon/cpython-wasi-build).
- **Setup complexity**: `pip install wasmtime` plus a CPython WASI build, a 13 MB download that unpacks to about 54 MB
    with the standard library; you manage the module, its precompilation and the stdlib directory yourself.
- **File mounting**: preopened directories.
- **Snapshotting**: not built in; a paused interpreter cannot be serialised, and pre-initialisation tools like
    [Wizer](https://github.com/bytecodealliance/wizer) only snapshot a module before it starts running.

## Sandboxing service

Services like [Daytona](https://daytona.io), [E2B](https://e2b.dev) and [Modal](https://modal.com).
Running your own sandbox setup on Kubernetes has similar characteristics, with more setup complexity but lower network
latency.

- **Language completeness**: full CPython with any library.
- **Security**: professionally managed container isolation.
- **Start latency**: a network round trip plus container startup.
    We measured 1.5 s to create a sandbox and run one line with Daytona EU from London, and about 40 ms per call to an
    existing sandbox; Daytona advertises sub-90 ms latency, presumably for the latter.
- **FOSS**: pay per execution or compute time; some implementations are open source.
- **Setup complexity**: API integration and auth tokens; fine for startups but often a non-starter for enterprises.
- **File mounting**: upload and download via API calls.
- **Snapshotting**: possible with durable execution solutions like Temporal; the services also offer their own
    solutions, generally based on container snapshots.

## YOLO Python

Running Python directly via `exec()` (about 0.1 ms) or a subprocess (about 30 ms).

- **Language completeness**: full CPython with any library.
- **Security**: none; full filesystem, network, environment variable and system command access.
- **Start latency**: near zero for `exec()`, about 30 ms for a subprocess.
- **Setup complexity**: none.
- **File mounting**: direct filesystem access, which is the problem.
- **Snapshotting**: possible with durable execution solutions like Temporal.
