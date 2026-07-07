import { spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import test from 'ava'

import { Monty, MontyCrashedError, MountDir } from '../ts/index.js'

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
    t.is(error.message, 'no monty worker became available within the checkout timeout')
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
  // Windows has no signals: process.kill('SIGKILL') calls TerminateProcess,
  // which is reported as a plain exit code of 1. Elsewhere the Rust
  // ExitStatus rendering includes the signal number.
  t.is(error.exitStatus, process.platform === 'win32' ? 'exit code: 1' : 'signal: 9 (SIGKILL)')
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
  t.is(error.message, 'RuntimeError: monty worker killed after exceeding request timeout of 500ms')
  await session.close()
})

// Reading a FIFO blocks the worker inside the OS, where the sandbox's
// periodic time check can never run — the host-side maxDurationSecs backstop
// (remaining budget + durationLimitGrace) is the only thing that can end the
// turn. Note no requestTimeout is configured. Unix-only (mkfifo).
const testOnUnix = process.platform === 'win32' ? test.skip : test
testOnUnix('duration backstop kills a worker blocked in a syscall', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'monty-fifo-'))
  try {
    t.is(spawnSync('mkfifo', [join(dir, 'pipe')]).status, 0)
    await using pool = await Monty.create({ durationLimitGrace: 0.3 })
    const session = await pool.checkout({ limits: { maxDurationSecs: 0.1 } })
    const error = await t.throwsAsync(
      () =>
        session.feedRun("from pathlib import Path\nPath('/mnt/pipe').read_text()", {
          mount: new MountDir('/mnt', dir, { mode: 'read-only' }),
        }),
      { instanceOf: MontyCrashedError },
    )
    t.true(error.timedOut)
    await session.close()
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

test('suspension time does not consume the duration budget', async (t) => {
  // maxDurationSecs measures cumulative sandbox execution time; the worker
  // reports it on every turn and its clock is paused while suspended. The
  // host taking twice the entire budget to answer an external call must
  // therefore not time the session out.
  await using pool = await Monty.create()
  await using session = await pool.checkout({ limits: { maxDurationSecs: 0.3 } })
  const result = await session.feedRun("await fetch_data('u') + '!'", {
    externalLookup: {
      fetch_data: async () => {
        await new Promise((resolve) => setTimeout(resolve, 600))
        return 'body'
      },
    },
  })
  t.is(result, 'body!')
})

// =============================================================================
// Environment isolation
// =============================================================================

// Workers must be spawned with an empty environment: host secrets must never
// be in a worker's memory, where a sandbox escape or memory disclosure could
// reach them. Linux-only because it observes the child via /proc (CI runs
// the JS tests on Linux).
const testOnLinux = process.platform === 'linux' ? test : test.skip
testOnLinux('worker environment is empty', async (t) => {
  t.truthy(process.env.PATH, 'test process should have PATH set')
  await using pool = await Monty.create()
  const session = await pool.checkout()
  const environ = await readFile(`/proc/${session.workerPid}/environ`)
  t.is(environ.length, 0, `worker environment should be empty, got: ${environ.toString().replaceAll('\0', ' ')}`)
  // The worker is fully functional without an environment.
  t.is(await session.feedRun('1 + 1'), 2)
  await session.close()
})
