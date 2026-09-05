# Security Model

Monty is designed to run code that a language model wrote and nobody reviewed.
This page describes what that buys you and what it does not.

The sandbox has been through two completed rounds of the [Hack Monty](https://pydantic.dev/articles/hack-monty-3)
bounty program, and a third is under way.
If you find a way out of it, please [open an issue](https://github.com/pydantic/monty/issues) or claim the bounty.

## What "secure" means here

Monty is a **language-level sandbox**, not an OS-level one.
There is no container, no seccomp filter and no VM.
The isolation comes from the interpreter itself: sandboxed code cannot express an operation that touches the host,
because the interpreter implements no such operation.

!!! note

    If you want Monty combined with OS-level isolation, see [Full Monty](server.md), the commercial version of Monty.

Concretely:

- **There is no ambient authority.** With no mounts and no host functions configured, the sandbox cannot read a file,
    read an environment variable, open a socket, or spawn a process.
    Not "it is blocked" — the capability does not exist in the bytecode VM.
    The wall clock is the one exception, and only for in-process Rust runs, which read it by default; see
    [the clock](#the-clock).
- **The interpreter performs no filesystem I/O at all.** It suspends with a description of the operation it wants, and a
    host component decides what to do about it.
    All filesystem code lives in a separate crate (`monty-fs`) that worker artifacts do not even link in some builds.
- **The dangerous modules are absent.** `socket`, `subprocess`, `multiprocessing`, `threading` and `ctypes`
    are not importable, and are also missing from the bundled typeshed, so [type checking](type-checking.md) rejects code
    that uses them before it runs.
- **No FFI, no C dependencies.** Nothing in the sandbox can call into native code.

## The three host-access mechanisms

Everything the sandbox can reach outside itself goes through one of three mechanisms, and all are opt-in per feed.

### Host functions

Names the sandbox does not define are resolved against the `external_lookup` you supply.
A callable entry becomes a function the sandbox can call: execution suspends, **your** code runs on the host with your
process's full authority, and execution resumes with the result.

=== "Python"

    ```python
    from pydantic_monty import Monty


    def get_price(sku: str) -> float:
        return {'A1': 3.5, 'B2': 12.0}[sku]


    with Monty() as pool:
        with pool.checkout() as session:
            result = session.feed_run(
                "get_price('B2') * 2", external_lookup={'get_price': get_price}
            )
            print(result)
            #> 24.0
    ```

=== "TypeScript"

    ```ts
    import { Monty } from '@pydantic/monty'

    function getPrice(sku: string): number {
      return { A1: 3.5, B2: 12.0 }[sku]!
    }

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    const result = await session.feedRun("get_price('B2') * 2", { externalLookup: { get_price: getPrice } })
    console.log(result) // 24
    ```

See [host functions](host-functions.md).

Monty guarantees that the sandbox reaches nothing you did not hand it.
It cannot guarantee that what you handed it is safe.
A host function that takes a path and reads it, or takes a URL and fetches it, is an unconstrained filesystem or network
primitive that you wrote.
Validate arguments in the host function as you would validate any untrusted input.

### Host objects and classes

[`ClassInstance`][pydantic_monty.ClassInstance] and [`ClassType`][pydantic_monty.ClassType] wrappers put a host object, or a host class, in front of the sandbox.
Every method call, lazy attribute read and `init=True` construction the wrapper allows runs **your** code on the host,
with the same authority as a host function.
`eager_attrs`, `lazy_attrs` and `allowed_methods` are name allow-lists that default to nothing, and `init` is a
boolean gate that defaults to `False`; `'all'` still skips underscore-prefixed names, and for `allowed_methods` it
exposes only the functions the class defines.
Nothing is wrapped for you: a method that returns another object fails conversion unless a `convert_value` hook wraps
it with a policy you chose.

=== "Python"

    ```python
    from dataclasses import dataclass

    from pydantic_monty import ClassInstance, Monty


    @dataclass
    class Account:
        owner: str
        balance: float

        def withdraw(self, amount: float) -> float:
            self.balance -= amount
            return self.balance

        def close(self) -> None: ...


    account = Account(owner='ada', balance=100.0)
    # the sandbox sees `owner` and `balance`, may call `withdraw`, and cannot call `close`
    wrapper = ClassInstance(
        account, eager_attrs={'owner', 'balance'}, allowed_methods={'withdraw'}
    )

    with Monty() as pool:
        with pool.checkout() as session:
            print(session.feed_run('account.withdraw(30)', inputs={'account': wrapper}))
            #> 70.0
    ```

=== "TypeScript"

    ```ts
    import { ClassInstance, Monty } from '@pydantic/monty'

    class Account {
      constructor(
        public owner: string,
        public balance: number,
      ) {}
      withdraw(amount: number): number {
        this.balance -= amount
        return this.balance
      }
      close(): void {}
    }

    const account = new Account('ada', 100)
    // the sandbox sees `owner` and `balance`, may call `withdraw`, and cannot call `close`
    const wrapper = new ClassInstance(account, { eagerAttrs: ['owner', 'balance'], allowedMethods: ['withdraw'] })

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    console.log(await session.feedRun('account.withdraw(30)', { inputs: { account: wrapper } })) // 70
    ```

See [host objects](host-objects.md).

### Mounts and the `os` callback

Host directories are mounted into the sandbox at virtual paths, and only inside a mount can `open()` and `pathlib` do
anything.

=== "Python"

    ```python
    import tempfile
    from pathlib import Path

    from pydantic_monty import Monty, MountDir

    with tempfile.TemporaryDirectory() as tmp:
        Path(tmp, 'notes.txt').write_text('mounted from the host')
        with MountDir(host_path=tmp, virtual_path='/data', mode='read-only') as mount:
            with Monty() as pool:
                with pool.checkout() as session:
                    print(session.feed_run("open('/data/notes.txt').read()", mount=mount))
                    #> mounted from the host
    ```

=== "TypeScript"

    ```ts
    import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
    import { tmpdir } from 'node:os'
    import { join } from 'node:path'

    import { Monty } from '@pydantic/monty'
    import { MountDir } from '@pydantic/monty/node'

    const tmp = mkdtempSync(join(tmpdir(), 'monty-'))
    writeFileSync(join(tmp, 'notes.txt'), 'mounted from the host')
    {
      using mount = new MountDir({ hostPath: tmp, virtualPath: '/data', mode: 'read-only' })
      await using pool = await Monty.create()
      await using session = await pool.checkout()
      console.log(await session.feedRun("open('/data/notes.txt').read()", { mount })) // mounted from the host
    }
    rmSync(tmp, { recursive: true })
    ```

A separate `os=` callback handles operations no mount covers: the remaining `pathlib` operations, `os.getenv`,
`os.environ`, `date.today()` and `datetime.now()`.
[`AbstractOS`][pydantic_monty.AbstractOS] is the typed form of that callback; [`OSAccess`][pydantic_monty.OSAccess] implements it over in-memory files and an `environ` mapping
you supply, and overriding one of its methods replaces one operation.
JavaScript has only the callback form, so the TypeScript tab answers the same three operations by hand:

=== "Python"

    ```python
    from datetime import datetime

    from pydantic_monty import MemoryFile, Monty, OSAccess


    class FrozenClock(OSAccess):
        def datetime_now(self, tz=None) -> datetime:
            return datetime(2026, 1, 1, 9, 30, tzinfo=tz)


    fs = FrozenClock(
        [MemoryFile('/config.json', content='{"stage": "test"}')], environ={'STAGE': 'test'}
    )
    code = """
    import json, os
    from datetime import datetime
    from pathlib import Path
    f'{os.getenv("STAGE")} {json.loads(Path("/config.json").read_text())["stage"]} {datetime.now():%H:%M}'
    """

    with Monty() as pool:
        with pool.checkout() as session:
            print(session.feed_run(code, os=fs))
            #> test test 09:30
    ```

=== "TypeScript"

    ```ts
    import { Monty, NOT_HANDLED, type MontyDateTime } from '@pydantic/monty'

    const files = new Map([['/config.json', '{"stage": "test"}']])
    const environ: Record<string, string> = { STAGE: 'test' }
    const frozenNow: MontyDateTime = {
      __monty_type__: 'DateTime',
      year: 2026,
      month: 1,
      day: 1,
      hour: 9,
      minute: 30,
      second: 0,
      microsecond: 0,
    }

    function fs(functionName: string, args: unknown[]) {
      if (functionName === 'Path.read_text') return files.get(args[0] as string) ?? NOT_HANDLED
      if (functionName === 'os.getenv') return environ[args[0] as string] ?? null
      if (functionName === 'datetime.now') return frozenNow
      return NOT_HANDLED
    }

    const code = `
    import json, os
    from datetime import datetime
    from pathlib import Path
    f'{os.getenv("STAGE")} {json.loads(Path("/config.json").read_text())["stage"]} {datetime.now():%H:%M}'
    `

    await using pool = await Monty.create()
    await using session = await pool.checkout()
    console.log(await session.feedRun(code, { os: fs })) // test test 09:30
    ```

See [filesystem access](filesystem.md).

Confinement is structural rather than checked:

- Each mount opens a `cap_std::fs::Dir` descriptor once, at mount time, and every operation runs relative to it — `..`,
    symlinks and directories swapped mid-operation cannot reach outside the mount, because no resolution step could leave
    it.
- `..` and `.` are collapsed in the virtual namespace before anything touches the filesystem.
- Symlinks with absolute targets are refused in read-only and read-write mounts, even when the target is inside the
    mount; overlay mounts refuse symlinks entirely.
- Null bytes in any path component are rejected.
- Paths handed back to the sandbox (from `Path.resolve()`, for example) are virtual paths.
    A host path never leaks in.

`/tmp`, `/etc`, `/proc`, `/dev`, `~` and the host working directory are not reachable unless you mount them.

### The clock

`date.today()` and `datetime.now()` are the only two calls that read a clock, and what answers them depends on how you
run the sandbox.

Through the pool — `pydantic_monty`, `@pydantic/monty`, or `monty-pool` — they reach your `os=` handler as OS calls like
any other, so the sandbox reads no clock until you write a handler that gives it one, and a handler that answers neither
makes both raise.

In-process Rust runs have no host loop to ask, so they read this machine's clock, as the `monty` CLI does.
`MontyRun::with_host_clock` changes that: `HostClock::Denied` if sandboxed code should not read your wall time at all,
`HostClock::Fixed` for a frozen instant.

Wall-clock time is a weak capability, but it is one — it is what makes elapsed time measurable from inside the sandbox,
and a naive `datetime.now()` is read in the host's local zone, which discloses its UTC offset.

## Crash isolation

Monty runs in a subprocess, so an unexpected memory error or panic in the interpreter cannot kill the main process;
the same design makes it easy to run many Monty interpreters in parallel.
The Python package and the native `@pydantic/monty` binding never run the interpreter in your process: every session
runs in a `monty` worker subprocess.

The WebAssembly build has no subprocess to use.
In a browser it runs off-thread in a `Worker`; under Node, which has no global `Worker`, `@pydantic/monty/wasm` runs
in-process outright.
See [in-process execution](#in-process-execution).

When a worker dies, the pool observes the death, discards the worker, spawns a replacement, and the call raises
[`MontyCrashedError`][pydantic_monty.MontyCrashedError] ([`PoolError::Crashed`](api/rust/monty-pool.md#poolerror) in Rust).
The session is lost; your process is not.

Two more properties of the worker boundary matter:

- **Workers spawn with an empty environment** (Windows keeps only `SystemRoot`), so host secrets are never in a worker's
    memory to begin with.
- **The parent treats every frame from a worker as untrusted input.** A worker could in principle be compromised, so
    wire decoding validates everything, enforces depth and size budgets, and never panics on malformed data.
    A worker that violates the protocol is discarded.

From Rust, this is why [`monty-pool`](quickstart/rust.md) is the recommended entry point rather than the in-process
`monty` crate.

## Resource exhaustion

Untrusted code will try to allocate forever or loop forever.
See [resource limits](resource-limits.md) for the full picture; the security-relevant parts:

- `max_memory` budgets the bytes a worker requests from its global allocator, not process RSS.
    Per-allocation overhead, fragmentation, and memory obtained without the allocator sit outside the count.
    Size the limit with headroom, and keep the worker-level backstop.
- `max_duration_secs` counts **cumulative execution time**, not wall clock.
    The clock is paused while the sandbox waits on a host function, so a slow host function does not consume the budget.
    It accumulates across feeds for the life of the session.
- The in-sandbox time check only runs at interpreter checkpoints.
    Two host-side backstops cover a wedged worker: `request_timeout` (a per-turn deadline; a loop of quick host calls
    resets it) and `duration_limit_grace` (fires only if the session also set `max_duration_secs`).
    Set both `request_timeout` and `max_duration_secs` for untrusted code.
    Every local pool ([`Monty`][pydantic_monty.Monty], [`AsyncMonty`][pydantic_monty.AsyncMonty], JavaScript `Monty.create()`, [`PoolConfig::subprocess`](api/rust/monty-pool.md#poolconfig)) defaults
    `request_timeout` to no deadline; only [`AsyncMontyWebsocket`][pydantic_monty.AsyncMontyWebsocket] sets one, at 10 seconds.
- **After a memory or time limit fires, no guarantees are made about heap state or reference counts.** Discard the
    session rather than continuing to run code in it.
    The pool does not do this for you, and the two limits do not even fail alike: a spent `max_duration_secs` budget is
    cumulative, so every later feed fails with the same `TimeoutError`, while after a `max_memory` trip a later feed may
    quietly succeed against a corrupted heap.
- Compilation is not charged against the duration budget.
    It has its own structural caps (AST nesting, bytecode operand sizes, comprehension nesting, `finally` expansion), but
    a host accepting untrusted source should still isolate compilation — as the subprocess and WebAssembly runtimes do.
- `max_suspensions` bounds suspension events per checkout.
    A snippet can otherwise retry a rejected host call while `max_duration_secs` is paused.
    Each allowed `ClassType(init=True)` construction adds an instance-store entry outside `max_memory`.
    The pool aborts the first suspension over the limit with an uncatchable `RuntimeError`.

## Where the guarantees weaken

### Your own callbacks

Host functions, the methods, lazy attributes and constructors exposed through [`ClassInstance`][pydantic_monty.ClassInstance]/[`ClassType`][pydantic_monty.ClassType], the `os=`
callback, and [`CallbackFile`][pydantic_monty.CallbackFile] in the Python [`OSAccess`][pydantic_monty.OSAccess] helper all execute in the host process.
`OSAccess` backed by [`MemoryFile`][pydantic_monty.MemoryFile] objects is fully sandboxed; `OSAccess` backed by `CallbackFile` is exactly as
sandboxed as the callback you wrote.

### In-process execution

The Rust `monty` crate and the WebAssembly in-process degrade run the interpreter in the calling process.
The language-level sandbox still holds, but crash isolation does not: an abort in the sandbox is an abort in your
process.
In the browser, a real `Worker` restores isolation and gives the watchdog a hard kill via `Worker.terminate()`; where no
`Worker` exists, the same API degrades to in-process with no preemption.

### Remote workers

[`AsyncMontyWebsocket`][pydantic_monty.AsyncMontyWebsocket] (Python) and [`PoolConfig::websocket`](api/rust/monty-pool.md#poolconfig) (Rust) dial a remote worker instead of spawning a local one.
**A remote peer need not be a Monty sandbox at all.** It may be real CPython with no sandbox, no resource limits and
full host access, relying on deployment isolation — a container or VM per session — rather than on the interpreter.
None of the guarantees on this page transfer across that boundary; they become properties of whatever is running on the
other end.

### Pin the worker binary

The worker binary is resolved from the explicit path you pass, then `MONTY_BIN`, then the bundled platform package, then
`PATH`.
When running untrusted code, pass the path explicitly rather than letting `PATH` decide which binary gets to be your
sandbox.

### Deserializing snapshots

[Snapshots](snapshots.md) are opaque bytes restored into a worker.
Treat a snapshot from an untrusted source the way you would treat any untrusted serialized data: restore it into a
worker you are willing to lose.

## The parts that are most security-critical

If you are reviewing or contributing to Monty, two files carry most of the weight:

- `crates/monty/src/heap.rs` — the heap and reference counting.
- `crates/monty-fs/src/mount_table.rs` — the mount boundary: the `Dir` descriptor every filesystem operation runs
    against, with `path_security.rs` beside it holding the virtual-path policy.

Changes to any of them need careful security review.
The repository's [`review-security` skill](https://github.com/pydantic/monty/tree/main/.agents/skills/review-security)
exists for exactly that.
