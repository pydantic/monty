# @pydantic/monty

Run untrusted Python safely from Node.js: a pool of crash-isolated `monty`
interpreter subprocesses, driven over a protobuf protocol by a pure-TypeScript
client.

[Monty](https://github.com/pydantic/monty) is a sandboxed Python interpreter
written in Rust. A sandbox process can never be made fully crash-proof against
memory errors (stack overflow, allocator aborts), so this package _only_ runs
the interpreter in worker subprocesses: a worker that crashes raises
`MontyCrashedError`, is replaced by the pool, and your Node.js process is
never at risk.

The `monty` binary ships via platform-specific npm packages installed
automatically (like esbuild). For browsers, see
[`@pydantic/monty-wasm`](https://www.npmjs.com/package/@pydantic/monty-wasm).

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

## External Functions

The sandbox can call host functions by name — sync or async (async functions
are awaited while other sandbox tasks keep running):

```ts
await session.feedRun('add(2, 3)', {
  externalFunctions: { add: (a: number, b: number) => a + b },
}) // 5

await session.feedRun('await fetch_data(url)', {
  inputs: { url: 'https://example.com' },
  externalFunctions: {
    fetch_data: async (url: string) => {
      const response = await fetch(url)
      return response.text()
    },
  },
})
```

Keyword arguments arrive as a trailing object; thrown errors cross into the
sandbox as Python exceptions (the error's `name` is used when it matches a
Python exception type, e.g. `TypeError`, otherwise `RuntimeError`).

## Print Output

```ts
await session.feedRun('print("hello")', {
  printCallback: (stream, text) => console.log(`[${stream}] ${text}`),
})
```

Output is line-buffered; without a callback it goes to the host process
stdout/stderr.

## Filesystem Mounts

Mount host directories into the sandbox at virtual POSIX paths:

```ts
import { MountDir } from '@pydantic/monty'

const mount = new MountDir('/mnt/data', '/path/on/host', { mode: 'read-only' })
await session.feedRun("open('/mnt/data/file.txt').read()", { mount })
```

Modes: `'read-only'`, `'read-write'`, and `'overlay'` (default — writes are
kept in worker memory and discarded at the end of the feed). OS calls mounts
don't cover can be handled with the `os` callback:

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
