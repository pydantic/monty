// Drives the REAL `MontySession.feedRun` loop over the wasm worker transport.
//
// `MontySession` is constructed with a structural `NativeSession`; here we
// inject a `WorkerTransport` backed by the lean wasip1 module under a WASI shim
// instead of a napi pool worker. This exercises the whole TypeScript drive loop
// — suspensions, external functions, prints, errors, session persistence —
// against the same Rust state machine the native subprocess runs, proving the
// browser path end to end.
//
// Requires the release wasm build:
//   cargo build -p monty-wasm-worker --target wasm32-wasip1 --release

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import type { NativeSession } from '../index.js'
import { MontyError } from '../ts/errors.js'
import { MontySession, NOT_HANDLED } from '../ts/session.js'
import { WasmHost, inProcessDispatcher } from '../ts/worker/host.js'
import { WorkerTransport } from '../ts/worker/transport.js'

// the module copied next to the loaders by scripts/copy-wasm.mjs
const wasmPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'ts', 'worker', 'monty_wasm_worker.wasm')

let wasmModule: WebAssembly.Module

test.before(async () => {
  wasmModule = await WebAssembly.compile(readFileSync(wasmPath))
})

/** A fresh session over its own wasm instance (isolated session state). */
async function session(): Promise<MontySession> {
  const host = await WasmHost.create(wasmModule)
  const transport = await WorkerTransport.create(inProcessDispatcher(host))
  return new MontySession(transport as unknown as NativeSession)
}

test('feed returns a value', async (t) => {
  const s = await session()
  t.is(await s.feedRun('1 + 2'), 3)
})

test('inputs are injected as globals', async (t) => {
  const s = await session()
  t.is(await s.feedRun('n + 1', { inputs: { n: 41 } }), 42)
})

test('session state persists across feeds', async (t) => {
  const s = await session()
  await s.feedRun('x = 10')
  t.is(await s.feedRun('x * 2'), 20)
})

test('print output is streamed to the callback', async (t) => {
  const s = await session()
  const chunks: string[] = []
  await s.feedRun("print('hi'); print('bye')", { printCallback: (_stream, text) => chunks.push(text) })
  t.is(chunks.join(''), 'hi\nbye\n')
})

test('external functions are called and their result flows back', async (t) => {
  const s = await session()
  const result = await s.feedRun('add_ints(2, 3) + 1', {
    externalFunctions: { add_ints: (a: number, b: number) => a + b },
  })
  t.is(result, 6)
})

test('containers round-trip through the value codec', async (t) => {
  const s = await session()
  t.deepEqual(await s.feedRun('[1, 2, 3]'), [1, 2, 3])
  t.deepEqual(
    await s.feedRun("{'a': 1, 'b': 2}"),
    new Map<unknown, unknown>([
      ['a', 1],
      ['b', 2],
    ]),
  )
})

test('the datetime family decodes to marked objects', async (t) => {
  const s = await session()
  t.deepEqual(await s.feedRun('import datetime; datetime.date(2024, 3, 1)'), {
    __monty_type__: 'Date',
    year: 2024,
    month: 3,
    day: 1,
  })
  t.deepEqual(await s.feedRun('import datetime; datetime.datetime(2024, 3, 1, 12, 30, 5, 7)'), {
    __monty_type__: 'DateTime',
    year: 2024,
    month: 3,
    day: 1,
    hour: 12,
    minute: 30,
    second: 5,
    microsecond: 7,
  })
  t.deepEqual(await s.feedRun('import datetime; datetime.timedelta(days=-1, seconds=3)'), {
    __monty_type__: 'TimeDelta',
    days: -1,
    seconds: 3,
    microseconds: 0,
  })
})

test('an aware datetime carries its offset and timezone name', async (t) => {
  const s = await session()
  const value = await s.feedRun(
    'import datetime\n' +
      "datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone(datetime.timedelta(hours=-5), 'EST'))",
  )
  t.like(value as object, { __monty_type__: 'DateTime', offsetSeconds: -18000, timezoneName: 'EST' })
})

test('datetime values round-trip as inputs', async (t) => {
  const s = await session()
  const date = { __monty_type__: 'Date', year: 2030, month: 12, day: 25 }
  // injected as a global, read back out — exercises both encode and decode
  t.deepEqual(await s.feedRun('d', { inputs: { d: date } }), date)
})

test('dataclass values round-trip as inputs', async (t) => {
  const s = await session()
  const dc = {
    __monty_type__: 'Dataclass',
    name: 'Pt',
    typeId: 99n,
    fieldNames: ['x', 'y'],
    fields: { x: 3, y: 4 },
    frozen: false,
  }
  t.deepEqual(await s.feedRun('d', { inputs: { d: dc } }), dc)
})

test('an external function referenced (not called) decodes to its name', async (t) => {
  const s = await session()
  t.is(await s.feedRun('foo', { externalFunctions: { foo: () => 1 } }), 'foo')
})

test('async external functions resolve through the futures path', async (t) => {
  const s = await session()
  const result = await s.feedRun('(await fetch_val()) + 1', {
    externalFunctions: { fetch_val: async () => 41 },
  })
  t.is(result, 42)
})

test('os callbacks answer OS calls, or fall through to the default error', async (t) => {
  const s = await session()
  const handled = await s.feedRun("import os\nos.getenv('HOME')", {
    os: (name) => (name === 'os.getenv' ? '/home/test' : NOT_HANDLED),
  })
  t.is(handled, '/home/test')

  const err = await t.throwsAsync(() => s.feedRun("import os\nos.getenv('HOME')"))
  t.is((err as MontyError).exception.typeName, 'RuntimeError')
})

test('dump returns the session snapshot bytes', async (t) => {
  const s = await session()
  await s.feedRun('x = 123')
  const state = await s.dump()
  t.true(state.length > 0)
})

test('a runtime error becomes a MontyRuntimeError', async (t) => {
  const s = await session()
  const err = await t.throwsAsync(() => s.feedRun('1 / 0'))
  t.true(err instanceof MontyError)
  t.is((err as MontyError).exception.typeName, 'ZeroDivisionError')
  t.is((err as MontyError).exception.message, 'division by zero')
})
