// Exercises the WorkerPool's elasticity/checkout/recycle/crash-replacement
// logic over an in-process wasm backend, driving real `MontySession`s.
//
// The per-turn watchdog (hard preemption via `Worker.terminate()`) is not
// covered here — it needs a real `Worker`; the in-process backend cannot
// interrupt a running turn. Everything else (reuse with state isolation,
// growth to max + waiting, recycle quota, crash → replacement) is.
//
// Requires the release wasm build:
//   cargo build -p monty-wasm-worker --target wasm32-wasip1 --release

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { MontyError } from '../ts/errors.js'
import { createWorkerPool } from '../ts/worker/index.js'
import type { PooledWorker, WorkerFactory } from '../ts/worker/pool.js'
import { WorkerPool, inProcessFactory } from '../ts/worker/pool.js'

// the module copied next to the loaders by scripts/copy-wasm.mjs
const wasmPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'ts', 'worker', 'monty_wasm_worker.wasm')

let wasmModule: WebAssembly.Module

test.before(async () => {
  wasmModule = await WebAssembly.compile(readFileSync(wasmPath))
})

/** Wraps a factory to count how many workers it spawns. */
function counting(inner: WorkerFactory): { factory: WorkerFactory; spawns: () => number } {
  let spawns = 0
  return {
    factory: () => {
      spawns++
      return inner()
    },
    spawns: () => spawns,
  }
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0))

test('a reused worker starts each session with clean state', async (t) => {
  const { factory, spawns } = counting(inProcessFactory(wasmModule))
  const pool = await WorkerPool.create(factory, { minWorkers: 1, maxWorkers: 2 })

  const s1 = await pool.checkout()
  await s1.feedRun('x = 5')
  await s1.close()

  const s2 = await pool.checkout()
  const err = await t.throwsAsync(() => s2.feedRun('x'))
  t.is((err as MontyError).exception.typeName, 'NameError')
  await s2.close()

  t.is(spawns(), 1, 'the second checkout reuses the first worker')
  await pool.close()
})

test('checkouts grow to maxWorkers, then wait for a release', async (t) => {
  const { factory, spawns } = counting(inProcessFactory(wasmModule))
  const pool = await WorkerPool.create(factory, { minWorkers: 1, maxWorkers: 2 })

  const s1 = await pool.checkout()
  const s2 = await pool.checkout()
  t.is(spawns(), 2, 'grew to maxWorkers')

  let third: Awaited<ReturnType<typeof pool.checkout>> | null = null
  const pending = pool.checkout().then((s) => (third = s))
  await tick()
  t.is(third, null, 'the third checkout waits while the pool is exhausted')

  await s1.close()
  await pending
  t.not(third, null, 'releasing a worker resolves the waiting checkout')
  t.is(spawns(), 2, 'the freed worker is reused, not a new spawn')

  await s2.close()
  await third!.close()
  await pool.close()
})

test('a worker is recycled after its checkout quota', async (t) => {
  const { factory, spawns } = counting(inProcessFactory(wasmModule))
  const pool = await WorkerPool.create(factory, {
    minWorkers: 1,
    maxWorkers: 1,
    maxCheckoutsPerWorker: 2,
  })

  for (let i = 0; i < 3; i++) {
    const s = await pool.checkout()
    t.is(await s.feedRun('1 + 1'), 2)
    await s.close()
  }

  t.is(spawns(), 2, 'the first worker served 2 checkouts then was recycled')
  await pool.close()
})

test('a crashed worker is discarded and replaced', async (t) => {
  // The first worker rejects its second dispatch (after ReplCreate), modelling
  // a worker that dies mid-feed; later workers are healthy.
  let spawned = 0
  const real = inProcessFactory(wasmModule)
  const factory: WorkerFactory = async () => {
    const worker = await real()
    if (spawned++ > 0) return worker
    let calls = 0
    const faulty: PooledWorker = {
      dispatch: (frame) => (++calls <= 1 ? worker.dispatch(frame) : Promise.reject(new Error('worker crashed'))),
      terminate: () => worker.terminate(),
      get alive() {
        return worker.alive
      },
    }
    return faulty
  }

  const pool = await WorkerPool.create(factory, { minWorkers: 1, maxWorkers: 1 })

  const s1 = await pool.checkout()
  await t.throwsAsync(() => s1.feedRun('1 + 1'), { instanceOf: MontyError })
  await s1.close()

  const s2 = await pool.checkout()
  t.is(await s2.feedRun('1 + 1'), 2, 'a fresh worker replaces the crashed one')
  await s2.close()

  t.is(spawned, 2, 'the crashed worker was discarded and one replacement spawned')
  await pool.close()
})

test('session resource limits are enforced inside the worker', async (t) => {
  const pool = await WorkerPool.create(inProcessFactory(wasmModule))
  // cooperative duration enforcement stops the loop without a watchdog
  const s = await pool.checkout({ limits: { maxDurationSecs: 0.1 } })
  const err = await t.throwsAsync(() => s.feedRun('while True:\n    pass'))
  t.true(err instanceof MontyError)
  await s.close()
  await pool.close()
})

test('createWorkerPool degrades to in-process where Worker is unavailable', async (t) => {
  const pool = await createWorkerPool(wasmModule)
  const s = await pool.checkout()
  t.is(await s.feedRun('6 * 7'), 42)
  await s.close()
  await pool.close()
})
