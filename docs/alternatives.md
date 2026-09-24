# Comparison to Alternatives

There are generally two responses when you show people Monty:

1. This solves so many problems, I want it.
1. Why not X?

Often both at once: people have not found an alternative that works for them, but doubt that writing a Python
implementation from scratch was the only option.

This page runs through the obvious alternatives and why they were not right for what we wanted: somewhere to run code
written by a model, per request, with nothing else in the loop.
Most of them were not designed as an LLM sandbox.

![Time to create a sandbox and run 10 REPL commands](img/startup-latency.svg)

The chart is the time to get a new sandbox and then run ten REPL commands in it; see [latency](#latency).

| Tech               | Language completeness      | Security          | Execution env | New sandbox           | FOSS       | Setup        | File mounting  | Snapshotting                |
| ------------------ | -------------------------- | ----------------- | ------------- | --------------------- | ---------- | ------------ | -------------- | --------------------------- |
| OSS Monty          | partial                    | strict            | local         | 0.8 ms, 5 ms new pool | free / OSS | easy         | easy           | interpreter, kilobytes      |
| Full Monty         | partial, or full via proxy | strict + OS-level | remote        | 2 ms, 3 ms new pool   | not free   | easy         | easy           | interpreter, kilobytes      |
| Docker             | full                       | good              | local         | 195 ms                | free / OSS | intermediate | easy           | CRIU image, experimental    |
| Pyodide            | full                       | poor              | local         | 2700 ms               | free / OSS | intermediate | easy           | no                          |
| starlark-rust      | very limited               | good              | local         | 1.3 ms                | free / OSS | easy         | not available? | no                          |
| WASI / wasmtime    | partial, almost full       | strict            | local         | 16 ms                 | free / OSS | intermediate | easy           | no                          |
| sandboxing service | full                       | strict            | remote        | 1500 ms               | not free   | intermediate | hard           | VM memory image, 100s of MB |
| YOLO Python        | full                       | non-existent      | local         | 0.1 ms / 30 ms        | free / OSS | easy         | easy / scary   | no                          |

Snapshotting means pausing code mid-execution, serialising its state, and resuming it later, possibly elsewhere or
more than once.
Only an interpreter built for it can do that at the interpreter level; a microVM can do it for its whole memory, at
a thousand times the size, and without knowing what the paused code was waiting for.
Durable-execution frameworks such as Temporal are not snapshotting: they replay a workflow written for them, which a
script a model just wrote is not.

Execution environment is where the code runs relative to the application that submits it.
A local option runs it on the application host, in the application process or in a child process, so an escape reaches
that host and the number of sandboxes is bounded by that host's memory and CPU.
A remote option runs it on other machines: an escape reaches those machines, each command costs a network round trip,
and the sandboxes scale by adding machines without touching the application hosts.
Docker is local as measured here, one daemon on the application host; run the daemon elsewhere, on Kubernetes or a
remote host, and it becomes a sandboxing service you operate.

## Latency

An application that runs model-written code asks two questions of its sandbox:

1. **New sandbox**: how long does it take to get a clean sandbox for a new user or session?
    Measured as the time from requesting one to receiving the result of `1 + 1`.
1. **Agent run**: how long does it take to run a series of commands in that sandbox?
    Measured as ten REPL commands against a sandbox that already exists.

For OSS Monty and Full Monty a new sandbox is a checkout from a pool the application created when it started: a fresh
REPL session on a pooled worker process.
Creating the pool happens once per application, not once per user, so it is shown separately.
The others have no pool, so each new sandbox starts from nothing.

| Sandbox                                      | New sandbox | Agent run, warm† | Combined |
| -------------------------------------------- | ----------- | ---------------- | -------- |
| OSS Monty, pool already open                 | 0.8 ms      | 0.4 ms           | 1.2 ms   |
| OSS Monty, new pool                          | 5 ms        | 0.4 ms           | 5 ms     |
| Full Monty, client pool already open         | 2 ms        | 5 ms             | 7 ms     |
| Full Monty, new client pool                  | 3 ms        | 5 ms             | 9 ms     |
| WASI / wasmtime, precompiled CPython         | 16 ms       | 180 ms           | 200 ms   |
| Docker, running container, `docker exec`     | 195 ms      | 700 ms           | 900 ms   |
| Sandboxing service, existing Daytona sandbox | 1500 ms     | 400 ms           | 1900 ms  |
| Pyodide, running Deno sandbox                | 2700 ms     | 35 ms            | 2700 ms  |

The chart uses the first row for each of OSS Monty and Full Monty.
Full Monty gives every session a fresh worker, so its two rows differ only on the client side: whether the pool
object and event loop already exist.

† 10 commands run in a REPL, as you might expect from a simple agent with code mode.
OSS Monty and Full Monty keep the session, so each command is one `feed_run`.
None of the others has a persistent interpreter to feed: `python.wasm` is a WASI command module whose `_start` runs
once, a container or a service runs one program per request, and the Pyodide sandbox evaluates each call in fresh
globals.
For those, command *n* re-runs commands 1 to *n*, the cheapest strategy that gives the same result, so the cost is ten
interpreter starts plus the replayed work.
The commands themselves are in `AGENT_BLOCKS` in the script: a list of orders, a function, comprehensions, `json`, and
an f-string report; every setup must print the same report.

Every number comes from
[`scripts/startup_performance.py`](https://github.com/pydantic/monty/blob/main/scripts/startup_performance.py).
Unless a section's "How it was measured" says otherwise, each was measured on 2026-09-03 on an Apple M3 Max (96 GB,
macOS 26.5.2) in London, from CPython 3.14.7, with a single sample per new sandbox.
The tables round the numbers; the measured values are in each section below.

## OSS Monty

- **Language completeness**: no class inheritance, limited stdlib, no third-party libraries.
    See [the Python subset](limitations/index.md).
- **Security**: no filesystem, network or environment access except what you pass in; limits on execution time and
    memory, off unless you set them.
    See the [security model](security.md).
- **Execution environment**: a `monty` worker subprocess per session, from a pool the application owns on its own host.
    An escape gives the code that worker process, which runs on the application host with the application's OS
    privileges.
    Capacity is the host's: each worker is about 2 MB baseline plus its memory limit, so one machine runs hundreds, and
    more hosts mean more pools.
- **New sandbox**: one message to a worker that already exists, 0.8 ms; creating the pool, once per application,
    spawns the worker and takes 5 ms.
- **Setup complexity**: `pip install pydantic-monty` or `npm install @pydantic/monty`, about 4.5 MB download.
- **File mounting**: strictly controlled, see [filesystem access](filesystem.md).
- **Snapshotting**: `feed_start()` pauses at a host call and `dump()` serialises the interpreter, paused call stack
    included, to a few kilobytes; restore it once to resume, or several times to fork.
    See [snapshots](snapshots.md).

### How it was measured

`pydantic-monty` with a release build of the `monty` worker binary, driven through [`Monty()`][pydantic_monty.Monty] /
`pool.checkout()` / `session.feed_run()`, the package's only execution API.

New sandbox is the median of 20 `checkout()` + `feed_run()` round trips against a pool whose worker already exists,
each after the pool has sat idle for a second, as it would while an agent waits on its model: 0.77 ms, measured on
2026-09-24 with `pydantic-monty` 1.0.0b3.
Back to back, with no idle time, a checkout takes 0.04 ms; the idle worker's caches go cold.

New pool creates the pool as well, which spawns the worker subprocess and completes the protocol handshake; the median
of 7 runs with `pydantic-monty` 0.0.21 is 4.5 ms.

The agent run is ten `feed_run` calls on one checkout, so state persists and nothing is replayed.

## Full Monty

[Full Monty](server.md) is the commercial server: the same `monty` workers behind a WebSocket, as a container image.

- **Language completeness**: the same subset as OSS Monty, or full CPython when the server proxies a session to a CPython
    sandbox.
- **Security**: the Monty sandbox plus OS-level isolation; escaping the sandbox reaches an empty container, not the
    machine running your application.
- **Execution environment**: the server's container, on whatever machines you deploy it to.
    Each connection gets a worker from the server's pool, and the server enforces its own session, memory and duration
    ceilings.
    The sandboxes scale by adding server replicas behind a load balancer, independent of the application hosts; a
    draining replica hands each session a signed dump so the client can resume on another.
    See [Full Monty](server.md).
- **New sandbox**: a WebSocket connection, relayed to a worker process for the session, 1.7 ms measured between
    containers on one machine; a deployment adds its network round trip.
- **FOSS**: closed-source and commercial; the client, [`AsyncMontyWebsocket`][pydantic_monty.AsyncMontyWebsocket], ships in the MIT `pydantic-monty`
    package.
- **Setup complexity**: run the container image with one environment variable, the dump-signing key.
- **File mounting**: client directories are mounted over the wire, the same [`MountDir`][pydantic_monty.MountDir] as a local pool.
- **Snapshotting**: as OSS Monty, and a draining server hands each session a signed dump to restore elsewhere.

### How it was measured

Measured on 2026-09-24: the production pair of containers, the Full Monty server relaying to a separate worker
container that runs one process per session, both native `linux/arm64` builds on monty 1.0.0-beta.2 in Docker
Desktop on the same machine.
The client is `pydantic-monty` 1.0.0b2's [`AsyncMontyWebsocket`][pydantic_monty.AsyncMontyWebsocket] in a third
container on the same Docker network, so the figures are the server's own overhead, not Docker Desktop's
port-forwarding proxy.
Each figure is the median of 7 runs.

New sandbox is the median of 20 `checkout()` + `feed_run()` round trips on a client pool that is already open, at
1.7 ms, measured back to back; each is a new connection and a new worker process, because the server never lets one
process serve two clients.
New client pool creates the client pool and opens the WebSocket connection as well, at 3.4 ms.

The agent run is ten `feed_run` calls on one checkout, 5.3 ms; each is a WebSocket round trip relayed from the server
to the same worker.
The worker runs inside a Linux container, where creating an OSS Monty pool measures 2.4 ms against 4.5 ms on macOS,
so the Full Monty numbers are not directly comparable with the macOS ones.

## Docker

- **Language completeness**: full CPython with any library.
- **Security**: process and filesystem isolation and network policies from the kernel; container escapes exist; cgroups
    cap memory.
- **Execution environment**: a container on the host running the Docker daemon, the application host as measured here.
    An escape reaches that host.
    Running the daemon on other machines, or containers on Kubernetes, moves the code off the application host at the
    cost of operating that cluster; see [sandboxing service](#sandboxing-service).
- **New sandbox**: 195 ms measured, the container start.
- **Setup complexity**: the Docker daemon, container images and orchestration; `python:3.14-alpine` is 50 MB and
    Docker cannot be installed from PyPI.
- **File mounting**: bind mounts and volumes.
- **Snapshotting**: not of a running process, except experimentally with CRIU on Linux; committing a container to an
    image saves its filesystem, not its execution state.

### How it was measured

Docker Desktop 29.6.2 with the `python:3.14-alpine` image already pulled.
New sandbox is `docker run --rm python:3.14-alpine python -c 'print(1 + 1)'`.
The agent run keeps one container alive (`docker run -d --rm python:3.14-alpine sleep infinity`) and executes each
replayed program with `docker exec <container> python -c ...`, so it pays for `docker exec` and a CPython start per
command but not for a container start.

## Pyodide

- **Language completeness**: full CPython compiled to WASM, almost all libraries available.
- **Security**: the browser/WASM sandbox, not designed for server-side isolation.
    Python code can run arbitrary code in the JS runtime; only Deno restricts what that runtime reaches, and memory
    limits are hard to enforce in Deno.
- **Execution environment**: a Deno process on the application host.
- **New sandbox**: 2700 ms measured, almost all of it loading the WASM runtime.
- **Setup complexity**: load the WASM runtime and handle async initialisation; the Pyodide npm package is about 12 MB
    and Deno about 50 MB, so Pyodide cannot be used with PyPI packages alone.
- **File mounting**: virtual filesystem via browser APIs.
- **Snapshotting**: no; a running Pyodide heap has no serialised form.

### How it was measured

[`mcp-run-python`](https://pypi.org/project/mcp-run-python/) 0.0.22, which starts a Deno 2.5.5 process running
Pyodide 0.28.2 and exposes it as an MCP server over stdio.
New sandbox is `code_sandbox()`, which spawns Deno and loads Pyodide, followed by one `eval`; installing a package such
as `numpy` at start adds about 200 ms more.
The agent run reuses a started sandbox and makes ten `eval` calls with the replayed programs; each call is an MCP round
trip into the already-loaded Pyodide, which keeps no globals between calls.

## starlark-rust

See [starlark-rust](https://github.com/facebook/starlark-rust).

- **Language completeness**: a configuration language, not Python; no classes, exceptions or async.
- **Security**: deterministic and hermetic by design.
- **Execution environment**: the application process.
- **New sandbox**: 1.3 ms for the first evaluation, around 0.01 ms after that.
- **Setup complexity**: usable from Python via [starlark-pyo3](https://github.com/inducer/starlark-pyo3).
- **File mounting**: no file handling by design, as far as we know.
- **Snapshotting**: no.

### How it was measured

[`starlark-pyo3`](https://pypi.org/project/starlark-pyo3/) 2026.1.1, in-process; the 1.3 ms is the first `parse` +
`eval` after import.
It has no agent-run row because the commands are Python, not Starlark.

## WASI / wasmtime

CPython compiled to WebAssembly (WASI), run by [wasmtime](https://wasmtime.dev/).

- **Language completeness**: almost full CPython; pure-Python packages work from a mounted directory, packages with C
    extensions need their own WASI build.
    In the WASI build `socket.socket()` and `subprocess.run()` raise `OSError`, `threading.Thread.start()` raises
    `RuntimeError`, and `ctypes` does not import.
- **Security**: the WebAssembly sandbox plus WASI's capability model; the guest sees only the directories and
    environment variables you preopen.
- **Execution environment**: the application process, through the `wasmtime` embedding.
- **New sandbox**: 16 ms with the module precompiled to a `.cwasm` file ahead of time, as a deployment would; about 95
    ms when wasmtime compiles from its cache and about 340 ms compiling from scratch.
- **Setup complexity**: `pip install wasmtime` plus a CPython WASI build, a 13 MB download that unpacks to about 54 MB
    with the standard library; you manage the module, its precompilation and the stdlib directory yourself.
- **File mounting**: preopened directories.
- **Snapshotting**: no; a paused interpreter cannot be serialised, and pre-initialisation tools like
    [Wizer](https://github.com/bytecodealliance/wizer) only snapshot a module before it starts running.

### How it was measured

The [CPython 3.14.7 WASI build](https://github.com/brettcannon/cpython-wasi-build) (`python.wasm` plus its `lib/`
directory, preopened as `/` with `PYTHONHOME=/`) run in-process through the
[`wasmtime`](https://pypi.org/project/wasmtime/) 48.0.0 Python package.
The module is compiled once to a `.cwasm` file ahead of time; the timed new sandbox deserialises it (about 1.5 ms),
instantiates, and runs `python -c 'print(1 + 1)'`, which is dominated by CPython's own startup inside the module.
The agent run deserialises once and creates one `Store` per command, replaying the earlier commands; deserialising a
new module while the previous store is still alive would add about 200 ms of page faults per command.

## Sandboxing service

Services like [Daytona](https://daytona.io), [E2B](https://e2b.dev) and [Modal](https://modal.com).
Running your own sandbox cluster on Kubernetes has the same characteristics, with more setup and lower network latency.

- **Language completeness**: full CPython with any library.
- **Security**: container or microVM isolation operated by the provider.
- **Execution environment**: the provider's machines.
    An escape reaches a machine in their fleet, the code and its inputs leave your network, and capacity is the
    provider's to add.
- **New sandbox**: a few network round trips plus sandbox creation.
    We measured 1.5 s to create a sandbox and run one line with Daytona EU from London, most of it Daytona creating
    the sandbox and running the first command rather than the network, and about 40 ms per call to an existing sandbox.
- **FOSS**: pay per execution or compute time; some implementations are open source.
- **Setup complexity**: API integration and auth tokens, plus a network dependency on the provider that some
    procurement policies rule out.
- **File mounting**: upload and download via API calls.
- **Snapshotting**: a microVM's whole memory can be paused and saved, which E2B and Modal offer as pause and resume;
    it is hundreds of megabytes, takes hundreds of milliseconds or more, is tied to the host's CPU and kernel, and
    the host cannot see what the paused code was waiting for.

### How it was measured

[Daytona](https://daytona.io) through the `daytona` 0.207.0 SDK, sandboxes in Daytona's EU region, called from London.
New sandbox is `Daytona().create()` followed by `sandbox.process.code_run("print(1 + 1)")`.
The agent run creates a sandbox, warms it with one call, then makes ten `code_run` calls with the replayed programs, so
each command is one HTTPS round trip plus a CPython start on the sandbox; the sandbox is deleted afterwards.

A new sandbox is three requests: `POST /api/sandbox`, a `GET` confirming the sandbox started, and the first `code_run`
through the EU sandbox proxy.
Re-measured on 2026-09-24 with the 0.216.1 SDK from Azure in London and Virginia, to remove the local network, sandbox
creation took 0.5–1.1 s and the first `code_run` 0.45–1 s.
The API is 7 ms from Virginia and the EU sandbox proxy, in Frankfurt, 20 ms from London, so the network explains at
most about 200 ms of the 1.5 s.
Daytona advertises sub-90 ms sandbox creation; we used `create()` with default arguments in the EU region and did not
reproduce it.

## YOLO Python

Running Python directly via `exec()` (about 0.1 ms) or a subprocess (about 30 ms).

- **Language completeness**: full CPython with any library.
- **Security**: none; full filesystem, network, environment variable and system command access.
- **Execution environment**: the application process, or a child of it with the same privileges.
- **New sandbox**: about 0.1 ms for `exec()`, about 30 ms for a subprocess.
- **Setup complexity**: none.
- **File mounting**: direct filesystem access, which is the problem.
- **Snapshotting**: no; `pickle` can save the globals between blocks, which is a session dump, not a paused frame.

### How it was measured

`eval("1 + 1")` in the measuring process and `python -c 'print(1 + 1)'` as a subprocess.
Replaying the agent run through ten subprocesses takes about 180 ms; ten `exec` calls into one namespace take 0.3 ms.
