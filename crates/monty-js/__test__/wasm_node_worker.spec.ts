// Drives the pool over REAL worker threads (Node `worker_threads`), proving the
// two things the in-process backend cannot: execution off the calling thread,
// and hard preemption of a runaway turn via `terminate()` (the watchdog).
//
// The browser uses a `Worker`-based factory instead of `nodeWorkerFactory`; the
// pool, transport and channel are identical.
//
// Requires the release wasm build:
//   cargo build -p monty-wasm --target wasm32-wasip1 --release

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { MontyCrashedError } from '../ts/errors.js'
import { nodeWorkerFactory } from '../ts/worker/nodeFactory.js'
import { WorkerPool } from '../ts/worker/pool.js'

// the module copied next to the loaders by scripts/copy-wasm.mjs
const wasmPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'ts', 'worker', 'monty_wasm.wasm')

let wasmModule: WebAssembly.Module

test.before(async () => {
  wasmModule = await WebAssembly.compile(readFileSync(wasmPath))
})

test('a feed runs in a real worker thread', async (t) => {
  const pool = await WorkerPool.create(nodeWorkerFactory(wasmModule))
  const s = await pool.checkout()
  t.is(await s.feedRun('40 + 2'), 42)
  await s.feedRun('y = 7')
  t.is(await s.feedRun('y * 3'), 21)
  await s.close()
  await pool.close()
})

test('the watchdog hard-kills a runaway turn and the pool recovers', async (t) => {
  const pool = await WorkerPool.create(nodeWorkerFactory(wasmModule, { requestTimeoutMs: 1000 }), {
    minWorkers: 1,
    maxWorkers: 1,
  })

  // an unbounded loop returns no turn-ending event; only terminate() stops it
  const runaway = await pool.checkout()
  await t.throwsAsync(() => runaway.feedRun('while True:\n    pass'), { instanceOf: MontyCrashedError })
  await runaway.close()

  // the killed worker was discarded; the pool spawns a healthy replacement
  const healthy = await pool.checkout()
  t.is(await healthy.feedRun('2 + 2'), 4)
  await healthy.close()

  await pool.close()
})
