# Getting Started with JavaScript

## Installation

```bash
npm install @pydantic/monty
```

Under Node, `@pydantic/monty` is a native (napi) binding over the same Rust worker pool the Python package uses.
The binding and the `monty` worker binary ship as platform-specific packages selected through `optionalDependencies`,
so a plain `npm install` gets you everything.
Execution happens in `monty` worker subprocesses, so a crash triggered by adversarial code kills only the worker.

For browsers, or anywhere subprocesses are impossible, the same package exposes a WebAssembly build under the
`@pydantic/monty/wasm` subpath, running in a Web Worker in browsers and a worker thread under Node; see
[browsers and WebAssembly](#browsers-and-webassembly).

## First run

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout({ limits: { maxMemory: 10_000_000, maxFeedDurationSecs: 1 } })

const result = await session.feedRun('double(x) + y', {
  inputs: { x: 5, y: 1 },
  externalLookup: { double: (x: number) => x * 2 },
})
console.log(result) // 11
```

`Monty.create()` spawns the pool and `pool.checkout()` dedicates one worker to one REPL session, with its resource
limits.
`feedRun` executes a snippet and returns the value of its trailing expression; `inputs` are values it can read and
`externalLookup` holds the host functions it can call.
`await using` closes the session and the pool at the end of scope.
Without it, call `session.close()` and `pool.close()` yourself.
`checkout({ scriptName })` names diagnostics and supplies the filename portion of the sandbox's `__file__`.

## Sessions keep state

Session state persists across `feedRun` calls on the same checkout:

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout()

await session.feedRun('x = 21')
console.log(await session.feedRun('x * 2')) // 42
```

## Getting values in

`inputs` binds values as globals eagerly.
`externalLookup` resolves names lazily when the sandbox reads them: a function entry becomes a [host
function](../host-functions.md) (sync or async), any other value is converted and returned on read, and a name absent
from the lookup raises `NameError` inside the sandbox.

Host functions may be async; the drive loop awaits them:

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout()

const data = await session.feedRun('await fetch_data()', {
  externalLookup: { fetch_data: async () => 'data' },
})
console.log(data) // data
```

Keyword arguments from the sandbox arrive as a trailing object on the call.
An error thrown by a host function crosses into the sandbox as a Python exception, using the error's `name` when it
matches a Python exception type and `RuntimeError` otherwise.

### Value conversion

| Python              | JavaScript                                      |
| ------------------- | ----------------------------------------------- |
| `None`              | `null`                                          |
| `bool`              | `boolean`                                       |
| `int`               | `number` within ±2^53, otherwise `BigInt`       |
| `float`             | `number`                                        |
| `str`               | `string`                                        |
| `bytes`             | `Buffer`                                        |
| `list`              | `Array`                                         |
| `tuple`             | `Array` with a non-enumerable `__tuple__: true` |
| `dict`              | `Map`                                           |
| `set` / `frozenset` | `Set`                                           |
| `datetime` family   | marker objects carrying `__monty_type__`        |
| builtin types       | `{ __monty_type__: 'Type', value }`             |
| builtin functions   | `{ __monty_type__: 'BuiltinFunction', value }`  |
| file handles        | `MontyFileHandle`                               |

Builtin type and function markers carry the builtin's name, never a JavaScript callable.
Passing a marker back resolves it to that builtin; unrecognized names are rejected with `unknown type name` or
`unknown builtin function`.

Plain objects with string keys are accepted as `dict` inputs.
Repeated references within one message preserve identity; see [host-value limitations](../limitations/host-values.md)
for cycles, deeply nested values and copies between calls.

## Host objects

Wrap a host object in `ClassInstance`, or a class in `ClassType`, to expose it under an explicit policy.
Nothing is wrapped automatically, so a method that returns another object needs a `convertValue` hook:

```ts
import { ClassInstance, ClassType, Monty } from '@pydantic/monty'

class Wallet {
  constructor(public balance: number) {}
  pay(amount: number) {
    return new Wallet(this.balance - amount)
  }
}

function wrapWallet(wallet: Wallet): ClassInstance {
  return new ClassInstance(wallet, {
    eagerAttrs: 'all',
    allowedMethods: 'all',
    convertValue: (_name, value) => (value instanceof Wallet ? wrapWallet(value) : value),
  })
}

await using pool = await Monty.create()
await using session = await pool.checkout()

console.log(await session.feedRun('w.pay(30).balance', { inputs: { w: wrapWallet(new Wallet(100)) } })) // 70
const WalletClass = new ClassType(Wallet, { init: true, instanceEagerAttrs: 'all' })
console.log(await session.feedRun('Wallet(5).balance', { inputs: { Wallet: WalletClass } })) // 5
```

Instances defined inside the sandbox arrive as read-only `MontyClassProxy` stand-ins.
See [host objects](../host-objects.md) for policies, identity and returned objects.

## Capturing printed output

```ts
import { CollectString, Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout()

const collector = new CollectString()
await session.feedRun("print('from the sandbox')", { printCallback: collector })
console.log(collector.output) // 'from the sandbox\n'
```

`CollectStreams` collects `(stream, text)` entries so you can tell stdout from stderr.
A plain `(stream, text) => void` callback works too.
Both collectors default to a 10 MiB cap (`DEFAULT_MAX_PRINT_COLLECT_BYTES`); `maxBytes: null` disables it for trusted hosts.
Other `maxBytes` values must be finite and non-negative.
Exceeding the cap rejects the feed with `MontyRuntimeError` wrapping `MemoryError`.
The cap is host-side and separate from [`maxMemory`](../resource-limits.md).
Without `printCallback`, Node writes to stdout/stderr; browsers send each output chunk to `console.log`/`console.error`.

Output arrives in batched chunks, not one per `print()`.
`printFlushInterval` on `checkout()` sets how long (in seconds) the worker may hold it — 0.005 by default, `0` for one
chunk per line — and output is always flushed before a host call and before a feed ends.

## OpenTelemetry instrumentation

Node applications can pass standard OpenTelemetry components directly, or register `MontyInstrumentation` with an
OpenTelemetry SDK:

```ts
import { NodeSDK } from '@opentelemetry/sdk-node'
import { MontyInstrumentation } from '@pydantic/monty/node'

const instrumentation = new MontyInstrumentation()
const sdk = new NodeSDK({
  instrumentations: [instrumentation],
})
sdk.start()

await instrumentation.forceFlush()
await sdk.shutdown()
```

Instrumentation is an explicit opt-in because it records source, inputs, outputs, host calls, exceptions, and printed
text.
It also records pool and execution metrics through the SDK's meter provider, without sandbox-supplied dimensions.
Configure instrumentation before creating pools.
`MontyInstrumentation.disable()` can stop telemetry while sessions remain active; changing providers or other signal
settings while pools are active is unsupported.
Drain Monty's callback queues with `instrumentation.forceFlush()` before shutting down the SDK.

For already configured OTel components, import `instrumentTelemetry` from `@pydantic/monty/node` and call
`instrumentTelemetry({ tracer, meter, logger })`; at least one component is required.
It applies process-wide and uses the providers' IDs, sampling, resources, metric views and exporters.
`MontyInstrumentation` obtains its logger through `@opentelemetry/api-logs`.
Call `flushTelemetry()` before flushing providers directly.
Worker threads wait for span creation; other records use bounded queues whose overflow disables the affected telemetry path.
This instrumentation is native-only; WASM still preserves caller context through `snapshot.traceContext()`.

## Errors

```ts
import { MontyError, MontyRuntimeError, MontySyntaxError, MontyCrashedError } from '@pydantic/monty'
```

| Class               | Raised when                                                              | Session survives |
| ------------------- | ------------------------------------------------------------------------ | ---------------- |
| `MontySyntaxError`  | The snippet does not parse                                               | yes              |
| `MontyTypingError`  | Type checking rejected the snippet                                       | yes              |
| `MontyRuntimeError` | The code raised at runtime                                               | yes              |
| `MontyCrashedError` | The worker died, or the watchdog killed it                               | no               |
| `ProtocolError`     | The worker, or a caller misusing the session, violated the wire protocol | no               |

`MontyError` is the base class of everything above except `ProtocolError`, which extends `Error`.
`err.exception` carries `{ typeName, message }`, and `err.display(format)` renders the error.
Which formats a class accepts differs; passing one a class does not accept throws, except `MontyTypingError.display()`,
which ignores any argument:

| Class                             | `display` formats                              |
| --------------------------------- | ---------------------------------------------- |
| `MontyError`, `MontyCrashedError` | `'msg'` (default), `'type-msg'`                |
| `MontySyntaxError`                | `'msg'` (default), `'type-msg'`, `'traceback'` |
| `MontyRuntimeError`               | `'traceback'` (default), `'type-msg'`, `'msg'` |
| `MontyTypingError`                | takes no argument; returns the diagnostics     |

`MontyCrashedError` adds `timedOut` and `exitStatus`.

## Limits and type checking

Both are per-session options on `checkout()`:

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout({
  limits: { maxMemory: 10_000_000, maxFeedDurationSecs: 1, maxRecursionDepth: 100 },
  typeCheck: true,
  typeCheckStubs: 'def fetch_data() -> str: ...',
})

console.log(await session.feedRun('fetch_data()', { externalLookup: { fetch_data: () => 'data' } })) // data
```

Omitted `maxMemory` / `maxFeedDurationSecs` means unlimited.
`maxFeedDurationSecs` and `maxTurnDurationSecs` bound one execution clock over one feed
(`feedRun` or `feedStart`) and one stretch of code between host round trips; each is unlimited when omitted.
`maxRecursionDepth` and `maxSuspensions` default to 1000 and cannot be disabled.
`gcInterval` defaults to every 100,000 allocations.
The pool enforces `maxSuspensions`: the first suspension over the budget ends the feed with an uncatchable
`RuntimeError`.
`typeCheckFormat` picks a ty diagnostic format and `typeCheckColor` colours it with ANSI escapes.
`assertMessageAnnotations: false` disables introspected assertion messages; an integer sets their truncation length.
See [assertions](../limitations/assert.md), [resource limits](../resource-limits.md) and [type checking](../type-checking.md).

Sessions default to the worker's clock and entropy, with sleeps capped at ten seconds per call.
For reproducible runs, `osPolicy` sets `datetime`, `timezone`, `sleep`, `sleepSystemMax` and `randomStart`:

```ts
import { Monty } from '@pydantic/monty'

const code = `
import random, time
from datetime import datetime
time.sleep(3600)
f'{datetime.now():%Y-%m-%d %H:%M} {random.random():.4f}'
`

await using pool = await Monty.create()
await using session = await pool.checkout({
  osPolicy: { datetime: new Date('2026-01-01T09:30:00Z'), sleep: 'zero', randomStart: { seed: 42 } },
})
console.log(await session.feedRun(code)) // 2026-01-01 09:30 0.6394
```

See [the clock](../security.md#the-clock).

## Filesystem mounts

`MountDir` is exported from the Node subpath, because mounts need a host filesystem:

```ts
import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { Monty } from '@pydantic/monty'
import { MountDir } from '@pydantic/monty/node'

await using pool = await Monty.create()
await using session = await pool.checkout()

using mount = new MountDir({ hostPath: mkdtempSync(join(tmpdir(), 'monty-')), virtualPath: '/data', mode: 'read-write' })
const text = await session.feedRun(
  "from pathlib import Path\np = Path('/data/new.txt')\np.write_text('hello')\np.read_text()",
  { mount },
)
console.log(text) // hello
```

`mode` is `'read-only'`, `'read-write'` or `'overlay'` (the default).
`using` closes the mount's directory handle at the end of scope, which Windows needs before the directory can be removed.
The sandbox's working directory is the first mount's virtual path, so `open('new.txt')` would reach the same file; the
`cwd` feed option picks another.
See [filesystem access](../filesystem.md).

## Configuring the pool

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create({
  binaryPath: undefined, // explicit path to the `monty` worker binary
  minProcesses: 1, // workers spawned up front
  maxProcesses: 8, // cap on live workers; defaults to the CPU count
  checkoutTimeout: 5, // seconds to wait for a free worker
  requestTimeout: 30, // hard per-turn deadline; kills the worker
  feedDurationLimitGrace: 1, // grace before the maxFeedDurationSecs backstop fires; null disables
  turnDurationLimitGrace: 1, // the same, for maxTurnDurationSecs
  maxCheckoutsPerWorker: 100, // recycle a worker after N sessions
})
```

Closing a pool rejects pending/new checkouts and reaps idle workers; checked-out sessions remain usable until closed.
`session.workerId` identifies the worker within its pool, including during turns and after the session closes.
Replacement workers get new IDs, even if the OS reuses a PID.
`maxCheckoutsPerWorker` accepts integers from 0 to 4294967295; 0 and 1 both retire a worker after each checkout.
`workerPid` is a native-only OS diagnostic and may be unavailable during a turn.

The worker binary is resolved from `binaryPath`, then the `MONTY_BIN` environment variable, then the installed platform
package, then `PATH`, and in a checkout of the Monty repository finally a cargo-built `target/` binary.

## Snapshots

`feedStart` is the suspendable counterpart of `feedRun`, returning a `Snapshot` at each suspension instead of driving to
completion.
`snapshot.resume(...)` returns the next snapshot or a `MontyComplete`; `snapshot.resumeAuto()` answers it from the
captured `externalLookup` / `os`.
A promise-returning external is awaited directly by `resumeAuto()` when the snapshot's `allowEagerAwait` is true, and
otherwise concurrently, surfacing as an intermediate `FutureSnapshot`, exactly as under `feedRun`.
Every snapshot's `position` is a `SourceRange` locating the suspending expression: the call, the name, or the `await`
the main task is blocked on (see [where execution stopped](../snapshots.md#where-execution-stopped)).
`snapshot.dump()` serializes a paused worker and `session.loadSnapshot(blob)` restores it; `session.dump()` and
`session.loadSession(blob)` do the same for an idle session between feeds.
Only restore unmodified snapshots from a trusted, compatible producer; the caller must establish provenance and integrity.
See [snapshot security](../security.md#deserializing-snapshots) before accepting bytes through an untrusted channel.

See [snapshots](../snapshots.md) for manual handlers, restoration and OpenTelemetry context propagation.

## Browsers and WebAssembly

Anywhere subprocesses are impossible, the same public API is available under `@pydantic/monty/wasm`, backed by a
WebAssembly build.
In a browser it runs in a Web Worker; under Node it uses `node:worker_threads`:

```ts test="skip"
import { Monty } from '@pydantic/monty/wasm'

await using pool = await Monty.create()
```

`Monty.create()` is `createWorkerPool(await loadModule())`, and both halves are exported: `loadModule()` fetches and
compiles the wasm modules, and `createWorkerPool(modules)` starts the workers, so an app can load the wasm ahead of time.
A bundler resolving the `browser` condition on the main entry point gets this build automatically.
[`examples/antigravity`](https://github.com/pydantic/monty/tree/main/examples/antigravity) is a worked browser example,
built with Vite.

Differences from the native path:

- **Filesystem mounts are unsupported** — a non-empty `mount` list is rejected, because there is no host filesystem.
- **`bytes` arrive as `Uint8Array`** wherever there is no `Buffer` global, which is every browser.
    Under Node the wasm build still hands back a `Buffer`.
- **Workers are required.** There is no in-process fallback; see [isolation guarantees](../security.md#crash-isolation).
    Startup has a separate 30-second deadline; request deadlines start after the worker is ready.
- **Browser `maxProcesses` defaults to `navigator.hardwareConcurrency`**, or 4 when unavailable; Node uses its CPU count.
- **`binaryPath` is ignored.** The bundled WASM asset is used instead.
- **OS diagnostics are backend-specific.** Browser crashes have no OS exit status.
    A hard allocator limit traps WASM and raises `MontyCrashedError`, not the native worker's classified `MemoryError`;
    see [allocator limits](../limitations/resource_limits.md#exceeding-max_memory-in-a-worker-pools).
- **Prints are buffered per turn** rather than streamed live.

## TypeScript reference

The package includes declarations with option and method documentation, maintained alongside the implementation:
[pool options](https://github.com/pydantic/monty/blob/main/crates/monty-js/ts/pool.ts),
[sessions and snapshots](https://github.com/pydantic/monty/blob/main/crates/monty-js/ts/session.ts),
[host objects](https://github.com/pydantic/monty/blob/main/crates/monty-js/ts/classInstance.ts),
[OS policies](https://github.com/pydantic/monty/blob/main/crates/monty-js/ts/options.ts) and
[value markers](https://github.com/pydantic/monty/blob/main/crates/monty-js/ts/types.ts).
Worker pools, channels and transports are internal; the WASM-specific public functions are `loadModule` and `createWorkerPool`.
