# @pydantic/monty

Run untrusted Python safely from JavaScript. In Node.js this uses a pool of
crash-isolated `monty` interpreter subprocesses; browser bundlers resolve the
same public API to a Web Worker pool backed by a lean wasm build.

[Monty](https://github.com/pydantic/monty) is a sandboxed Python interpreter
written in Rust. A sandbox process can never be made fully crash-proof against
memory errors (stack overflow, allocator aborts), so the native binding
(`@pydantic/monty`, `@pydantic/monty/node`) only runs the interpreter in worker
subprocesses. A worker that crashes raises `MontyCrashedError` and is replaced
by the pool. The wasm entry (`@pydantic/monty/wasm`, and the `browser` export)
runs off-thread in a `Worker` in browsers, but in-process under Node, which has
no global `Worker`.

The native binding and the `monty` binary ship together via platform-specific
npm packages installed automatically (like esbuild). Browser builds use the
package `browser` export and never import the napi loader; they run the sandbox
in a Web Worker as a WIT-defined WASI 0.2 component with the same pool/session
API. Advanced Node-only helpers are available from `@pydantic/monty/node`, and wasm-specific
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

## Class Instances

Wrap a host object in `ClassInstance` to send it into the sandbox. The wrapper
is a policy: it decides which attributes cross eagerly (`eagerAttrs`), which
the sandbox may fetch lazily on demand (`lazyAttrs`), and which methods it may
call (`allowedMethods`) — each an explicit list/`Set`, or `'all'`, which never
exposes `_`-prefixed names. Method calls and lazy lookups route back to the
real object, and when sandbox code returns the instance, the host receives the
**original object** back (identity preserved):

```ts
import { ClassInstance } from '@pydantic/monty'

class Wallet {
  constructor(public balance: number) {}
  pay(amount: number) {
    return new Wallet(this.balance - amount)
  }
}

const wallet = new Wallet(100)
await session.feedRun('w.pay(30).balance', {
  inputs: { w: new ClassInstance(wallet, { eagerAttrs: 'all', allowedMethods: 'all' }) },
}) // 70
```

Methods may be sync or async (`await w.fetch()` in the sandbox). JS functions
have no keyword arguments, so kwargs arrive as a trailing options-bag object.
Names outside the policy raise `AttributeError` in the sandbox. A
`convertValue` option hook transforms each value crossing to the sandbox
(eager attrs, lazy lookup results, method returns); the default auto-wraps
returned class instances with the same policies. Unwrapped class instances are
rejected with a `TypeError` — wrapping is always explicit.

Two more options: `name` overrides the class name the sandbox sees (default
`instance.constructor.name`), and `frozen: true` makes in-sandbox `setattr`
raise `FrozenInstanceError`. `frozen` defaults to `false`, so by default
sandbox code may set attributes — on its own copy only: sandbox mutations
never touch the wrapped host object.

Instances the host has no original for — defined inside the sandbox, or
returned after a dump was restored into a fresh process — surface as read-only
`MontyClassInstance` proxies (`name`, `attributes`, `isDataclass`).

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
import { MountDir } from '@pydantic/monty/node'

const mount = new MountDir({ hostPath: '/path/on/host', virtualPath: '/mnt/data', mode: 'read-only' })
await session.feedRun("open('/mnt/data/file.txt').read()", { mount })
```

Each mount has a 100 MB aggregate memory budget by default. Configure it with
`memoryUsageLimit`; retained overlay data and filesystem results share it, and
operations that exceed it raise a `MontyRuntimeError` wrapping `MemoryError`.

Modes: `'read-only'`, `'read-write'`, and `'overlay'` (default — writes are
kept in memory and discarded at the end of the feed). Mount I/O is serviced
on the host side of the pool, so mounts work even for remote workers.

The constructor opens the host directory, so an unusable path throws there
rather than at the first feed, and the mount then follows _that directory_ for
its lifetime — renaming or replacing it afterwards changes nothing.

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

Callback-backed virtual files return a `MontyFileHandle` marker from the
open-time call. Paths are virtual POSIX sandbox paths and `position` defaults
to zero:

```ts
import { MontyFileHandle, NOT_HANDLED } from '@pydantic/monty'

const files = new Map([['/data/message.txt', 'hello from the host']])
await session.feedRun("open('/data/message.txt').read()", {
  os: (name, args) => {
    const path = args[0] as string
    if (name === 'open') {
      return new MontyFileHandle(path, args[1] as string)
    }
    if (name === 'Path.read_text') return files.get(path) ?? NOT_HANDLED
    return NOT_HANDLED
  },
})
```

`MontyFileHandle` canonicalizes `mode` and exposes the same file metadata as
the Python host API: `path`, `mode`, `position`, `binary`, `readable`, and
`writable`. Pass a nonzero initial position with
`new MontyFileHandle(path, mode, { position: 42 })`.

Returning the handle resolves only `open()` itself. Reads and writes are
separate OS callbacks whose first argument is the handle's virtual path; the
host never receives or exposes a live file descriptor.

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
    console.log(err.display()) // rendered diagnostics
  }
}
```

A snippet that fails type checking does not run; the session survives.

`typeCheckFormat` picks the rendering — ty's `'full'` (the default: source
snippet and carets), `'concise'`, `'azure'`, `'json'`, `'jsonlines'`,
`'rdjson'`, `'pylint'`, `'gitlab'` or `'github'` — and `typeCheckColor` adds
ANSI colour to `'full'` and `'concise'`. Both are checkout options rather than
`display()` arguments because the diagnostics are rendered inside the worker:
ty's structured diagnostics resolve their spans against the type checker's
database, so only the rendered text crosses the wire.

```ts
const session = await pool.checkout({ typeCheck: true, typeCheckFormat: 'json' })
```

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

A session's `maxMemory` is enforced in the worker's own allocator too (the
[`monty-alloc`](https://crates.io/crates/monty-alloc) crate), which bounds the
bytes the worker holds at once — allocated minus freed, plus headroom — instead
of letting it grow the host without bound. A
worker that cannot honour the limit raises `MontyRuntimeError` wrapping
`MemoryError` — but unlike other runtime errors it takes the worker with it, so
the session is finished (the pool recovers). The wasm worker applies the same
limit to what it allocates, but a trapped module has no exit status to classify,
so there it raises `MontyCrashedError`.

The `monty` binary resolves from: explicit `binaryPath` → the `MONTY_BIN`
environment variable → the installed platform package → `PATH` → a cargo
workspace `target/` build (development).

The Node-only Logfire integration installs a version-1 adapter through
`_installTelemetryAdapter(1, adapter)`. At checkout it propagates the active
host trace context into Monty's exporter-free Rust spans, then reconstructs
those records through the host SDK, which owns credentials, export, and
shutdown. Delivery uses a bounded non-blocking queue; overflow permanently
disables the adapter and sends one global cleanup notification rather than
risking unbounded host memory. Browser/WASM does not yet implement this adapter
path.

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
| file handles      | `MontyFileHandle`                                       |
| class instances   | `ClassInstance` wrappers / `MontyClassInstance` proxies |

Plain objects are accepted as dict inputs (string keys).
