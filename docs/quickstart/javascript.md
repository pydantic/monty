# JavaScript QuickStart

```bash
npm install @pydantic/monty
```

Under Node, `@pydantic/monty` is a native (napi) binding over the same Rust worker pool
the Python package uses. Execution happens in `monty` worker subprocesses, so a crash
triggered by adversarial code kills only the worker.

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout()

console.log(await session.feedRun('1 + 2')) // 3
```

`await using` closes the session and the pool at the end of scope. Without it, call
`session.close()` and `pool.close()` yourself.

## Sessions keep state

Session state persists across `feedRun` calls on the same checkout:

```ts
await session.feedRun('x = 21')
console.log(await session.feedRun('x * 2')) // 42
```

## Getting values in

`inputs` binds values as globals eagerly. `externalLookup` resolves names lazily when the
sandbox reads them: a function entry becomes a [host function](../host-functions.md)
(sync or async), any other value is converted and returned on read, and a name absent
from the lookup raises `NameError` inside the sandbox.

```ts
const result = await session.feedRun('double(x) + y', {
  inputs: { x: 5, y: 1 },
  externalLookup: { double: (x: number) => x * 2 },
})
console.log(result) // 11
```

Host functions may be async; the drive loop awaits them:

```ts
const data = await session.feedRun('await fetch_data()', {
  externalLookup: { fetch_data: async () => 'data' },
})
```

Keyword arguments from the sandbox arrive as a trailing object on the call. An error
thrown by a host function crosses into the sandbox as a Python exception, using the
error's `name` when it matches a Python exception type and `RuntimeError` otherwise.

### Value conversion

| Python | JavaScript |
| --- | --- |
| `None` | `null` |
| `bool` | `boolean` |
| `int` | `number` within ±2^53, otherwise `BigInt` |
| `float` | `number` |
| `str` | `string` |
| `bytes` | `Buffer` |
| `list` | `Array` |
| `tuple` | `Array` with a non-enumerable `__tuple__: true` |
| `dict` | `Map` |
| `set` / `frozenset` | `Set` |
| `datetime` family | marker objects carrying `__monty_type__` |
| file handles | `MontyFileHandle` |

Plain objects with string keys are accepted as `dict` inputs.

## Capturing printed output

```ts
import { CollectString } from '@pydantic/monty'

const collector = new CollectString()
await session.feedRun("print('from the sandbox')", { printCallback: collector })
console.log(collector.output) // 'from the sandbox\n'
```

`CollectStreams` collects `(stream, text)` entries so you can tell stdout from stderr. A
plain `(stream, text) => void` callback works too. Both collectors default to a 10 MiB
cap (`DEFAULT_MAX_PRINT_COLLECT_BYTES`); pass `null` to disable it. The cap is host-side
and separate from [`maxMemory`](../resource-limits.md).

## Errors

```ts
import { MontyError, MontyRuntimeError, MontySyntaxError, MontyCrashedError } from '@pydantic/monty'
```

| Class | Raised when | Session survives |
| --- | --- | --- |
| `MontySyntaxError` | The snippet does not parse | yes |
| `MontyTypingError` | Type checking rejected the snippet | yes |
| `MontyRuntimeError` | The code raised at runtime | yes |
| `MontyCrashedError` | The worker died, or the watchdog killed it | no |
| `ProtocolError` | The worker, or a caller misusing the session, violated the wire protocol | no |

`MontyError` is the base class of everything above except `ProtocolError`, which extends
`Error`. `err.exception` carries `{ typeName, message }`, and `err.display(format)`
renders the error. Which formats a class accepts differs, and passing one a class does not
accept throws:

| Class | `display` formats |
| --- | --- |
| `MontyError`, `MontyCrashedError` | `'msg'` (default), `'type-msg'` |
| `MontySyntaxError` | `'msg'` (default), `'type-msg'`, `'traceback'` |
| `MontyRuntimeError` | `'traceback'` (default), `'type-msg'`, `'msg'` |
| `MontyTypingError` | takes no argument; returns the diagnostics |

`MontyCrashedError` adds `timedOut` and `exitStatus`.

## Limits and type checking

Both are per-session options on `checkout()`:

```ts
await using session = await pool.checkout({
  limits: { maxMemory: 10_000_000, maxDurationSecs: 1, maxRecursionDepth: 100 },
  typeCheck: true,
  typeCheckStubs: 'def fetch_data() -> str: ...',
})
```

`ResourceLimits` fields are `maxDurationSecs`, `maxMemory`, `gcInterval` and
`maxRecursionDepth`; an omitted field means unlimited, except `maxRecursionDepth`, which
falls back to its 1000-frame default and cannot be disabled. See
[resource limits](../resource-limits.md) and [type checking](../type-checking.md).

## Filesystem mounts

`MountDir` is exported from the Node subpath, because mounts need a host filesystem:

```ts
import { MountDir } from '@pydantic/monty/node'

const mount = new MountDir({ hostPath: '/tmp/data', virtualPath: '/data', mode: 'read-write' })
const text = await session.feedRun(
  "from pathlib import Path\np = Path('/data/new.txt')\np.write_text('hello')\np.read_text()",
  { mount },
)
```

`mode` is `'read-only'`, `'read-write'` or `'overlay'` (the default). See
[filesystem access](../filesystem.md).

## Configuring the pool

```ts
await using pool = await Monty.create({
  binaryPath: undefined, // explicit path to the `monty` worker binary
  minProcesses: 1, // workers spawned up front
  maxProcesses: 8, // cap on live workers; defaults to the CPU count
  checkoutTimeout: 5, // seconds to wait for a free worker
  requestTimeout: 30, // hard per-turn deadline; kills the worker
  durationLimitGrace: 1, // grace before the maxDurationSecs backstop fires; null disables
  maxCheckoutsPerWorker: 100, // recycle a worker after N sessions
})
```

The worker binary is resolved from `binaryPath`, then the `MONTY_BIN` environment
variable, then the installed platform package, then `PATH`.

## Snapshots

`feedStart` is the suspendable counterpart of `feedRun`, returning a `Snapshot` at each
suspension instead of driving to completion. `snapshot.resume(...)` returns the next
snapshot or a `MontyComplete`; `snapshot.resumeAuto()` answers it from the captured
`externalLookup` / `os`. `snapshot.dump()` serializes a paused worker and
`session.loadSnapshot(blob)` restores it; `session.dump()` and `session.loadSession(blob)`
do the same for an idle session between feeds.

See [snapshots](../snapshots.md) for the model, which is identical to Python's.

## Browsers and WebAssembly

Anywhere subprocesses are impossible, the same public API is available under
`@pydantic/monty/wasm`, backed by a Web Worker running a WebAssembly build:

```ts
import { Monty } from '@pydantic/monty/wasm'

const pool = await Monty.create()
```

A bundler resolving the `browser` condition on the main entry point gets this build
automatically.

Differences from the native path:

- **Filesystem mounts are unsupported** — a non-empty `mount` list is rejected, because
  there is no host filesystem.
- **`bytes` arrive as `Uint8Array`** wherever there is no `Buffer` global, which is every
  browser. Under Node the wasm build still hands back a `Buffer`.
- **No crash isolation without `Worker`.** Where a real `Worker` exists, it runs
  off-thread and `Worker.terminate()` is the watchdog's hard kill. Where one does not,
  the same API degrades to in-process execution: no crash isolation and no preemption, so
  a runaway turn cannot be interrupted.
- **`maxProcesses` defaults to 4**, not the CPU count.
- **`checkoutTimeout`, `durationLimitGrace` and `binaryPath` are accepted and ignored.**
  A checkout on an exhausted pool waits forever rather than failing, nothing backs up
  `maxDurationSecs` from outside the worker, and the bundled wasm asset is always used.
  `requestTimeout` does apply, wherever a real `Worker` exists.
- **Prints are buffered per turn** rather than streamed live, and rendered traceback
  strings are not produced yet (frames still decode).

Full API documentation lives in the
[package README](https://github.com/pydantic/monty/tree/main/crates/monty-js).
