# @pydantic/monty

Run untrusted Python safely from JavaScript. In Node.js this uses a pool of
crash-isolated `monty` interpreter subprocesses; browser bundlers resolve the
same public API to a Web Worker pool backed by a lean wasm build.

[Monty](https://github.com/pydantic/monty) is a sandboxed Python interpreter
written in Rust. A sandbox process can never be made fully crash-proof against
memory errors (stack overflow, allocator aborts), so this package _only_ runs
the interpreter in worker subprocesses: a worker that crashes raises
`MontyCrashedError`, is replaced by the pool, and your Node.js process is
never at risk.

The native binding and the `monty` binary ship together via platform-specific
npm packages installed automatically (like esbuild). Browser builds use the
package `browser` export and never import the napi loader; they run the sandbox
in a Web Worker (`wasm32-wasip1`) with the same pool/session API. Advanced
Node-only helpers are available from `@pydantic/monty/node`, and wasm-specific
factories from `@pydantic/monty/wasm`.

## Installation

```bash
npm install @pydantic/monty
```

## Basic Usage

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout()

const result = await session.feedRun('1 + 2') // 3
```

A session is a REPL in a dedicated worker — state persists across feeds:

```ts
await session.feedRun('x = 21')
await session.feedRun('x * 2') // 42
```

Without `await using`, call `session.close()` (returns the worker to the pool)
and `pool.close()` explicitly.

## Inputs

Pass values as globals for a feed:

```ts
await session.feedRun('x + y', { inputs: { x: 10, y: 20 } }) // 30
```

## External Lookup

`externalLookup` resolves names a snippet leaves undefined, lazily and on
demand. A **function** entry becomes a host function the sandbox can call by
name — sync or async (async functions are awaited while other sandbox tasks
keep running). Any **other value** is converted and returned directly when the
name is read. An absent name raises `NameError`.

```ts
await session.feedRun('add(2, 3)', {
  externalLookup: { add: (a: number, b: number) => a + b },
}) // 5

await session.feedRun('await fetch_data(url)', {
  inputs: { url: 'https://example.com' },
  externalLookup: {
    fetch_data: async (url: string) => {
      const response = await fetch(url)
      return response.text()
    },
  },
})

await session.feedRun('greeting + name', {
  inputs: { name: 'Ada' },
  externalLookup: { greeting: 'hello ' },
}) // 'hello Ada'
```

`externalLookup` is the lazy counterpart to `inputs`, which eagerly binds every
entry as a global whether or not it is referenced; a name in both is served by
the eager `inputs` binding.

For function entries, keyword arguments arrive as a trailing object; thrown
errors cross into the sandbox as Python exceptions (the error's `name` is used
when it matches a Python exception type, e.g. `TypeError`, otherwise
`RuntimeError`).

## Snapshots: pausing and resuming

`feedStart` is the suspendable counterpart of `feedRun`: instead of driving a
snippet to completion, it returns a snapshot at each external call, OS call, or
name lookup. Answer it with `snapshot.resume(...)`, which resolves to the next
snapshot or a `MontyComplete`.

```ts
import { FunctionSnapshot, MontyComplete } from '@pydantic/monty'

const snap = await session.feedStart('greet(name) + "!"', { inputs: { name: 'Ada' } })
if (snap instanceof FunctionSnapshot) {
  // snap.functionName === 'greet', snap.args === ['Ada']
  const done = await snap.resume('hello Ada')
  if (done instanceof MontyComplete) console.log(done.output) // 'hello Ada!'
}
```

To iterate a snippet to completion without answering each suspension by hand,
pass an `externalLookup` (and/or `os`) to `feedStart` and drive with
`snapshot.resumeAuto()`, which resolves each external call and name lookup from
them automatically — the same resolution `feedRun` performs, but one step at a
time so you can inspect or `dump()` each snapshot along the way. A
promise-returning external is awaited concurrently (surfacing as an intermediate
`FutureSnapshot`), exactly as under `feedRun`:

```ts
let snap = await session.feedStart('greet(name) + "!"', {
  inputs: { name: 'Ada' },
  externalLookup: { greet: (n: string) => `hello ${n}` },
})
while (!(snap instanceof MontyComplete)) {
  snap = await snap.resumeAuto()
}
console.log(snap.output) // 'hello Ada!'
```

`snapshot.dump()` serializes the paused worker to bytes; a fresh session's
`loadSnapshot` restores it and returns the snapshot to resume. Re-supply the
same `mount`s the paused feed used — their host paths are not stored in the
dump.

```ts
const blob = await snap.dump()
// ...later, in a fresh session:
const restored = await session.loadSnapshot(blob)
if (restored instanceof FunctionSnapshot) await restored.resume('value')
```

`session.dump()` between feeds serializes an idle session instead; restore it
with `await session.loadSession(blob)` (which resolves to `void`) and keep
feeding. Both `loadSession` and `loadSnapshot` are valid only on a fresh
session, before any feed; using the wrong one for a dump's kind throws.

## Print Output

`printCallback` accepts a function or a host collector (`PrintTargetInput` in
TypeScript). Output is line-buffered; without a callback it goes to the host
process stdout/stderr.

```ts
// Function form
await session.feedRun('print("hello")', {
  printCallback: (stream, text) => console.log(`[${stream}] ${text}`),
})

// Collectors — accumulate on the host (not covered by ResourceLimits.maxMemory)
import { CollectString, CollectStreams, DEFAULT_MAX_PRINT_COLLECT_BYTES } from '@pydantic/monty'

const text = new CollectString()
await session.feedRun('print("hello")', { printCallback: text })
text.output // 'hello\n'

const streams = new CollectStreams()
await session.feedRun('print("hello")', { printCallback: streams })
streams.output // [{ stream: 'stdout', text: 'hello\n' }]
```

Both collectors default to a **10 MiB** cap (`DEFAULT_MAX_PRINT_COLLECT_BYTES`).
Pass `maxBytes: null` to disable (trusted hosts only). `maxBytes` must be a
finite non-negative number or `null` (constructors throw `TypeError` otherwise).
Exceeding the cap rejects the feed with `MontyRuntimeError` / `MemoryError`
(`memory limit exceeded: …`).

## Filesystem Mounts

Mount host directories into the sandbox at virtual POSIX paths:

```ts
import { MountDir } from '@pydantic/monty'

const mount = new MountDir({ hostPath: '/path/on/host', virtualPath: '/mnt/data', mode: 'read-only' })
await session.feedRun("open('/mnt/data/file.txt').read()", { mount })
```

Each mount has a 100 MB aggregate memory budget by default. Configure it with
`memoryUsageLimit`; retained overlay data and filesystem results share it, and
operations that exceed it raise a `MontyRuntimeError` wrapping `MemoryError`.

Modes: `'read-only'`, `'read-write'`, and `'overlay'` (default — writes are
kept in memory and discarded at the end of the feed). Mount I/O is serviced
on the host side of the pool, so mounts work even for remote workers.

`feedRun` answers every OS call automatically: mounts get first refusal, then
the `os` callback. `feedStart` answers none — a mounted read surfaces as a
`FunctionSnapshot` with `isOsFunction` set, and `resumeAuto()` is what consults
the mounts and `os`. OS calls mounts don't cover reach the `os` callback:

```ts
import { NOT_HANDLED } from '@pydantic/monty'

await session.feedRun('import os\nos.getenv("HOME")', {
  os: (name, args) => (name === 'os.getenv' && args[0] === 'HOME' ? '/home/user' : NOT_HANDLED),
})
```

## Resource Limits

Enforced inside the worker, configured per session:

```ts
const limited = await pool.checkout({
  limits: { maxMemory: 100 * 1024 * 1024, maxDurationSecs: 5, maxRecursionDepth: 100 },
})
```

`requestTimeout` on the pool is the backstop for code that wedges the
interpreter itself: the worker is killed and the session fails with
`MontyCrashedError` (`timedOut: true`).

`maxDurationSecs` limits cumulative _execution_ time: the sandbox clock runs
only while the interpreter executes, never while suspended waiting on an
external function or between feeds. Sessions with the limit also get an
automatic backstop: the worker reports its execution time on every protocol
turn and the host kills it `durationLimitGrace` (default 1s) after the
remaining budget expires, covering cases where the in-sandbox limit cannot
fire (its check only runs at interpreter checkpoints). Set
`durationLimitGrace: null` to disable it.

## Assert message annotations

Failed `assert` statements carry a pytest-style introspected message by
default (`AssertionError: assert 2 == 5`) — a deliberate divergence from
CPython's empty `AssertionError`. Each operand's repr is truncated to 120
characters by default. Disable the messages per session to restore CPython's
behavior, or pass an integer to customize the truncation length:

```ts
const session = await pool.checkout({ assertMessageAnnotations: false })
const verbose = await pool.checkout({ assertMessageAnnotations: 1000 })
```

## Type Checking

```ts
import { MontyTypingError } from '@pydantic/monty'

const session = await pool.checkout({ typeCheck: true, typeCheckStubs: 'def fetch(url: str) -> str: ...' })
try {
  await session.feedRun('fetch(123)')
} catch (err) {
  if (err instanceof MontyTypingError) {
    console.log(err.display()) // rendered diagnostics, one per line
  }
}
```

A snippet that fails type checking does not run; the session survives.

## Error Handling

```ts
import { MontyError, MontySyntaxError, MontyRuntimeError, MontyCrashedError } from '@pydantic/monty'

try {
  await session.feedRun('1 / 0')
} catch (err) {
  if (err instanceof MontyRuntimeError) {
    console.log(err.exception.typeName) // 'ZeroDivisionError'
    console.log(err.display('traceback')) // full Python-style traceback
  }
}
```

`MontyError` is the base class; `MontyCrashedError` means the worker process
died (the session is lost, the pool recovers).

## Pool Configuration

```ts
const pool = await Monty.create({
  minProcesses: 1, // prewarmed workers
  maxProcesses: 8, // cap; checkouts beyond it wait (default: CPU count)
  checkoutTimeout: 10, // seconds to wait for a free worker
  requestTimeout: 30, // hard per-turn deadline (seconds)
  durationLimitGrace: 1, // maxDurationSecs backstop grace (seconds, null disables)
  maxCheckoutsPerWorker: 100, // recycle workers after this many sessions
  binaryPath: '/path/to/monty', // explicit binary (default: auto-resolved)
})
```

The `monty` binary resolves from: explicit `binaryPath` → the `MONTY_BIN`
environment variable → the installed platform package → `PATH` → a cargo
workspace `target/` build (development).

## Value Conversion

| Python            | JavaScript                                              |
| ----------------- | ------------------------------------------------------- |
| `None`            | `null`                                                  |
| `bool`            | `boolean`                                               |
| `int`             | `number` (±2^53) or `BigInt`                            |
| `float`           | `number`                                                |
| `str`             | `string`                                                |
| `bytes`           | `Buffer`                                                |
| `list`            | `Array`                                                 |
| `tuple`           | `Array` with non-enumerable `__tuple__: true`           |
| `dict`            | `Map` (preserves key types and order)                   |
| `set`/`frozenset` | `Set`                                                   |
| datetime types    | marker objects (`{ __monty_type__: 'DateTime', ... }`)  |
| dataclasses       | marker objects (`{ __monty_type__: 'Dataclass', ... }`) |

Plain objects are accepted as dict inputs (string keys).
