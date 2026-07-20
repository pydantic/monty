import { test } from 'vitest'
import { spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

import { Monty, MontyCrashedError, MontyRuntimeError, MountDir } from '@pydantic/monty/node'

// =============================================================================
// Pool lifecycle
// =============================================================================

test('checkout after close rejects', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  await pool.close()
  const error = await t.throwsAsync(() => pool.checkout())
  t.is(error.message, 'the pool is closed — create a new Monty pool')
})

test('close is idempotent', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  await pool.close()
  await pool.close()
  t.pass()
})

test('feed after session close rejects', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create()
  const session = await pool.checkout()
  await session.close()
  const error = await t.throwsAsync(() => session.feedRun('1'))
  t.is(error.message, 'the session is closed — check out a new one')
})

test('workers are reused across checkouts', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create({ maxProcesses: 1 })
  const first = await pool.checkout()
  const pid = first.workerPid
  t.truthy(pid)
  await first.close()
  const second = await pool.checkout()
  t.is(second.workerPid, pid)
  await second.close()
})

test('maxCheckoutsPerWorker recycles the worker', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create({ maxCheckoutsPerWorker: 1 })
  const first = await pool.checkout()
  const pid = first.workerPid
  await first.close()
  const second = await pool.checkout()
  t.not(second.workerPid, pid)
  await second.close()
})

test('concurrent sessions run in distinct workers', async (ctx) => {
  skipIfBrowser(ctx)
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

test('exhausted pool times out the checkout', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create({ maxProcesses: 1, checkoutTimeout: 0.2 })
  const held = await pool.checkout()
  try {
    const error = await t.throwsAsync(() => pool.checkout())
    t.is(error.message, 'no monty worker became available within the checkout timeout')
  } finally {
    await held.close()
  }
})

test('released worker is handed to a waiting checkout', async (ctx) => {
  skipIfBrowser(ctx)
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

test('killed worker surfaces as MontyCrashedError', async (ctx) => {
  skipIfBrowser(ctx)
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

test('session is unusable after a crash but the pool recovers', async (ctx) => {
  skipIfBrowser(ctx)
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

test('worker crashing while idle is replaced transparently', async (ctx) => {
  skipIfBrowser(ctx)
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

test('requestTimeout kills a wedged worker', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create({ requestTimeout: 0.5 })
  const session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('while True:\n    pass'), {
    instanceOf: MontyCrashedError,
  })
  t.true(error.timedOut)
  t.is(error.message, 'RuntimeError: monty worker killed after exceeding request timeout of 500ms')
  await session.close()
})

// Mount I/O runs on the host side of the pool, so reading a FIFO must fail
// fast (a real read would block the host with no watchdog able to rescue it).
// The sandbox sees a catchable PermissionError and the session stays usable.
// Unix-only (mkfifo).
test('special files in mounts are rejected without blocking', async (ctx) => {
  skipIfBrowser(ctx)
  if (process.platform === 'win32') {
    ctx.skip()
  }
  const dir = await mkdtemp(join(tmpdir(), 'monty-fifo-'))
  try {
    t.is(spawnSync('mkfifo', [join(dir, 'pipe')]).status, 0)
    await using pool = await Monty.create()
    const session = await pool.checkout()
    const error = await t.throwsAsync(
      () =>
        session.feedRun("from pathlib import Path\nPath('/mnt/pipe').read_text()", {
          mount: new MountDir('/mnt', dir, { mode: 'read-only' }),
        }),
      { instanceOf: MontyRuntimeError },
    )
    t.is(error.message, "PermissionError: [Errno 13] Permission denied: '/mnt/pipe'")
    await session.close()
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

test('suspension time does not consume the duration budget', async (ctx) => {
  skipIfBrowser(ctx)
  // maxDurationSecs measures cumulative sandbox execution time; the worker
  // reports it on every turn and its clock is paused while suspended. The
  // host taking twice the entire budget to answer an external call must
  // therefore not time the session out.
  await using pool = await Monty.create()
  await using session = await pool.checkout({ limits: { maxDurationSecs: 0.3 } })
  const result = await session.feedRun("await fetch_data('u') + '!'", {
    externalLookup: {
      fetch_data: async (ctx) => {
        skipIfBrowser(ctx)
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
test('worker environment is empty', async (ctx) => {
  skipIfBrowser(ctx)
  if (process.platform !== 'linux') {
    ctx.skip()
  }
  t.truthy(process.env.PATH, 'test process should have PATH set')
  await using pool = await Monty.create()
  const session = await pool.checkout()
  const environ = await readFile(`/proc/${session.workerPid}/environ`)
  t.is(environ.length, 0, `worker environment should be empty, got: ${environ.toString().replaceAll('\0', ' ')}`)
  // The worker is fully functional without an environment.
  t.is(await session.feedRun('1 + 1'), 2)
  await session.close()
})
