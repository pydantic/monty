import test from 'ava'

import { Monty, MontyCrashedError } from '../src/index.js'

// =============================================================================
// Pool lifecycle
// =============================================================================

test('checkout after close rejects', async (t) => {
  const pool = await Monty.create()
  await pool.close()
  const error = await t.throwsAsync(() => pool.checkout())
  t.is(error.message, 'the pool is closed — create a new Monty pool')
})

test('close is idempotent', async (t) => {
  const pool = await Monty.create()
  await pool.close()
  await pool.close()
  t.pass()
})

test('feed after session close rejects', async (t) => {
  await using pool = await Monty.create()
  const session = await pool.checkout()
  await session.close()
  const error = await t.throwsAsync(() => session.feedRun('1'))
  t.is(error.message, 'the session is closed — check out a new one')
})

test('workers are reused across checkouts', async (t) => {
  await using pool = await Monty.create({ maxProcesses: 1 })
  const first = await pool.checkout()
  const pid = first.workerPid
  t.truthy(pid)
  await first.close()
  const second = await pool.checkout()
  t.is(second.workerPid, pid)
  await second.close()
})

test('maxCheckoutsPerWorker recycles the worker', async (t) => {
  await using pool = await Monty.create({ maxCheckoutsPerWorker: 1 })
  const first = await pool.checkout()
  const pid = first.workerPid
  await first.close()
  const second = await pool.checkout()
  t.not(second.workerPid, pid)
  await second.close()
})

test('concurrent sessions run in distinct workers', async (t) => {
  await using pool = await Monty.create()
  const a = await pool.checkout()
  const b = await pool.checkout()
  try {
    t.not(a.workerPid, b.workerPid)
    const [ra, rb] = await Promise.all([a.feedRun('1 + 1'), b.feedRun('2 + 2')])
    t.is(ra, 2)
    t.is(rb, 4)
  } finally {
    await a.close()
    await b.close()
  }
})

test('exhausted pool times out the checkout', async (t) => {
  await using pool = await Monty.create({ maxProcesses: 1, checkoutTimeout: 0.2 })
  const held = await pool.checkout()
  try {
    const error = await t.throwsAsync(() => pool.checkout())
    t.is(error.message, 'no worker became available within 0.2s')
  } finally {
    await held.close()
  }
})

test('released worker is handed to a waiting checkout', async (t) => {
  await using pool = await Monty.create({ maxProcesses: 1 })
  const held = await pool.checkout()
  const waiting = pool.checkout()
  await held.close()
  const session = await waiting
  t.is(await session.feedRun('40 + 2'), 42)
  await session.close()
})

// =============================================================================
// Crash isolation
// =============================================================================

test('killed worker surfaces as MontyCrashedError', async (t) => {
  await using pool = await Monty.create()
  const session = await pool.checkout()
  process.kill(session.workerPid!, 'SIGKILL')
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyCrashedError })
  t.false(error.timedOut)
  t.is(error.exitStatus, 'signal: SIGKILL')
})

test('session is unusable after a crash but the pool recovers', async (t) => {
  await using pool = await Monty.create()
  const session = await pool.checkout()
  process.kill(session.workerPid!, 'SIGKILL')
  await t.throwsAsync(() => session.feedRun('1'), { instanceOf: MontyCrashedError })
  // Subsequent calls fail fast with the same error.
  await t.throwsAsync(() => session.feedRun('1'), { instanceOf: MontyCrashedError })
  await session.close()
  // The pool replaced the worker; new checkouts work.
  const fresh = await pool.checkout()
  t.is(await fresh.feedRun('1 + 1'), 2)
  await fresh.close()
})

test('worker crashing while idle is replaced transparently', async (t) => {
  await using pool = await Monty.create({ maxProcesses: 1 })
  const first = await pool.checkout()
  const pid = first.workerPid!
  await first.close()
  process.kill(pid, 'SIGKILL')
  // Give the OS a moment to reap it.
  await new Promise((resolve) => setTimeout(resolve, 100))
  const second = await pool.checkout()
  t.not(second.workerPid, pid)
  t.is(await second.feedRun('1 + 1'), 2)
  await second.close()
})

// =============================================================================
// Request timeout watchdog
// =============================================================================

test('requestTimeout kills a wedged worker', async (t) => {
  await using pool = await Monty.create({ requestTimeout: 0.5 })
  const session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('while True:\n    pass'), {
    instanceOf: MontyCrashedError,
  })
  t.true(error.timedOut)
  t.is(error.message, 'RuntimeError: the worker process was killed because a request timed out')
  await session.close()
})
