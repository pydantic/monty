import { test } from 'vitest'
import { spawnSync } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { t } from './assertions.js'
import { skipIfWasm } from './env.js'
import { WorkerTransport } from '../ts/worker/transport.js'
import { MontyCrashedError as TransportCrashedError } from '../ts/errors.js'
import { WorkerPool } from '../ts/worker/pool.js'
import { WorkerChannel, type WorkerMessage } from '../ts/worker/channel.js'
import type { Event as ComponentEvent } from '../ts/worker/component/monty.component.js'

import { Monty, MontyCrashedError, MontyRuntimeError } from '@pydantic/monty'
import { MountDir } from '@pydantic/monty/node'

// =============================================================================
// Pool lifecycle
// =============================================================================

test('wasm transport discards shutdown replies but preserves fatal diagnostics', async () => {
  const cases: Array<{ events: ComponentEvent[]; message: string }> = [
    { events: [], message: 'worker exited without a turn-ending event' },
    { events: [{ tag: 'ok' }], message: 'worker exited without a turn-ending event' },
    {
      events: [{ tag: 'complete', val: { values: { nodes: [{ tag: 'none' }] }, value: 0 } }],
      message: 'worker exited without a turn-ending event',
    },
    { events: [{ tag: 'fatal-error', val: 'specific failure' }], message: 'specific failure' },
  ]
  for (const { events, message } of cases) {
    const requests: string[] = []
    const transport = await WorkerTransport.create(async (request) => {
      requests.push(request.tag)
      return request.tag === 'configure'
        ? { status: 'continue', events: [{ tag: 'ok' }], feedExecutionMicros: 0n }
        : { status: 'shutdown', events, feedExecutionMicros: 0n, exitStatus: 'exit code: 7' }
    })
    const releases: boolean[] = []
    transport.onFinish = (reusable) => {
      releases.push(reusable)
    }
    t.deepEqual(await transport.feed('1', null, [], { skipTypeCheck: true }, () => {}), {
      kind: 'crashed',
      message,
      timedOut: false,
      exitStatus: 'exit code: 7',
    })
    await transport.finish()
    t.deepEqual(releases, [false])
    t.deepEqual(requests, ['configure', 'feed'])
  }
})

test('shutdown during Configure preserves the fatal diagnostic', async () => {
  const error = await t.throwsAsync(
    () =>
      WorkerTransport.create(async () => ({
        status: 'shutdown',
        events: [{ tag: 'fatal-error', val: 'configuration failed' }],
        feedExecutionMicros: 0n,
        exitStatus: 'exit code: 7',
      })),
    { instanceOf: TransportCrashedError },
  )
  t.is(error.message, 'RuntimeError: configuration failed')
  t.is(error.exitStatus, 'exit code: 7')
})

test.each(['acquire', 'startup', 'configure'])('close cancels a checkout during %s', async (phase) => {
  let start!: () => void
  const started = new Promise<void>((resolve) => {
    start = resolve
  })
  let terminations = 0
  let message: (reply: WorkerMessage) => void = () => {}
  const pool = await WorkerPool.create(
    (signal) =>
      WorkerChannel.create(
        {
          post: () => {
            start()
          },
          onMessage: (handler) => {
            message = handler
            if (phase === 'startup') start()
            else queueMicrotask(() => handler({ ready: true }))
          },
          onError: () => {},
          terminate: () => {
            terminations++
          },
        },
        {},
        signal,
      ),
    { minWorkers: phase === 'acquire' ? 1 : 0, maxWorkers: 1 },
  )
  const pending = t.throwsAsync(() => pool.checkout())
  if (phase !== 'acquire') await started
  await pool.close()
  t.is((await pending).message, 'the pool is closed — create a new Monty pool')
  message({ ready: true })
  await pool.close()
  t.is(terminations, 1)
})

test('failed direct and queued spawns wake the next checkout', async () => {
  let attempts = 0
  await using pool = await WorkerPool.create(
    async () => {
      if (++attempts <= 2) throw new Error(`spawn ${attempts} failed`)
      return {
        alive: true,
        terminate() {},
        async dispatch() {
          return { status: 'continue', events: [{ tag: 'ok' }], feedExecutionMicros: 0n }
        },
      }
    },
    { minWorkers: 0, maxWorkers: 1, checkoutTimeoutMs: 1000 },
  )
  const first = t.throwsAsync(() => pool.checkout())
  const second = t.throwsAsync(() => pool.checkout())
  const [firstError, secondError] = await Promise.all([
    first,
    second,
    pool.checkout().then((session) => session.close()),
  ])
  t.is(firstError.message, 'spawn 1 failed')
  t.is(secondError.message, 'spawn 2 failed')
  t.is(attempts, 3)
})

test.each([-1, 0.5, NaN, Infinity, 2 ** 32, 2 ** 32 + 1])(
  'rejects invalid recycle count %s',
  async (maxCheckoutsPerWorker) => {
    const error = await t.throwsAsync(() => Monty.create({ minProcesses: 0, maxCheckoutsPerWorker }))
    t.is(error.message, 'maxCheckoutsPerWorker must be an integer between 0 and 4294967295')
  },
)

test('queued checkout snapshots nested options before waiting', async () => {
  await using pool = await Monty.create({ maxProcesses: 1 })
  const held = await pool.checkout()
  const options = {
    scriptName: 'original.py',
    limits: { maxRecursionDepth: 100 },
    osPolicy: { datetime: new Date('2000-01-01T00:00:00Z'), randomStart: { seed: new Uint8Array([97, 98, 99]) } },
  }
  const waiting = pool.checkout(options)
  options.scriptName = 'mutated.py'
  options.limits.maxRecursionDepth = 1
  options.osPolicy.datetime.setUTCFullYear(2020)
  options.osPolicy.randomStart.seed.fill(9)
  await held.close()
  await using session = await waiting
  t.deepEqual(
    await session.feedRun(`
from datetime import datetime
import random
def count(n):
    return count(n - 1) + 1 if n else 0
(datetime.now().year, __file__, random.random() == random.Random(b'abc').random(), count(10))
`),
    [2000, '/original.py', true, 10],
  )
})

test('checkout after close rejects', async () => {
  const pool = await Monty.create()
  await pool.close()
  const error = await t.throwsAsync(() => pool.checkout())
  t.is(error.message, 'the pool is closed — create a new Monty pool')
})

test('close is idempotent', async () => {
  const pool = await Monty.create()
  await pool.close()
  await pool.close()
  t.pass()
})

test('close rejects waiting checkouts but preserves active sessions', async () => {
  const pool = await Monty.create({ maxProcesses: 1 })
  await using session = await pool.checkout()
  const waiting = t.throwsAsync(() => pool.checkout())
  await pool.close()
  t.is((await waiting).message, 'the pool is closed — create a new Monty pool')
  t.is(await session.feedRun('40 + 2'), 42)
})

test('feed after session close rejects', async () => {
  await using pool = await Monty.create()
  const session = await pool.checkout()
  await session.close()
  const error = await t.throwsAsync(() => session.feedRun('1'))
  t.is(error.message, 'the session is closed — check out a new one')
})

test.each([undefined, 0xffffffff])(
  'workers are reused across checkouts (recycle count %s)',
  async (maxCheckoutsPerWorker) => {
    await using pool = await Monty.create({ maxProcesses: 1, maxCheckoutsPerWorker })
    const first = await pool.checkout()
    const id = first.workerId
    t.true(Number.isSafeInteger(id))
    await first.feedRun('f()', {
      externalLookup: {
        f: () => {
          t.is(first.workerId, id)
          return null
        },
      },
    })
    await first.close()
    t.is(first.workerId, id)
    const second = await pool.checkout()
    t.is(second.workerId, id)
    await second.close()
  },
)

test.each([0, 1])('maxCheckoutsPerWorker %s recycles the worker', async (maxCheckoutsPerWorker) => {
  await using pool = await Monty.create({ maxCheckoutsPerWorker })
  const first = await pool.checkout()
  const id = first.workerId
  await first.close()
  const second = await pool.checkout()
  t.not(second.workerId, id)
  await second.close()
})

test('maxMemory leaves normal work alone', async () => {
  // a session's limit must not disturb work that stays inside it
  await using pool = await Monty.create()
  const session = await pool.checkout({ limits: { maxMemory: 1024 ** 2 } })
  t.is(await session.feedRun('1 + 1'), 2)
  await session.close()
})

test('a refused allocation raises MemoryError and the pool recovers', async (ctx) => {
  skipIfWasm(ctx)
  await using pool = await Monty.create()
  const session = await pool.checkout()
  // no maxMemory, so the sandbox tracker allows this outright: the allocation is
  // refused below the interpreter, killing the worker but still reporting
  // MemoryError rather than an unclassifiable crash
  const error = await t.throwsAsync(() => session.feedRun("x = ' ' * (1 << 60)"), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'MemoryError: the worker exceeded its memory limit and was terminated')
  t.is(error.exception.typeName, 'MemoryError')
  const next = await pool.checkout()
  t.is(await next.feedRun('1 + 1'), 2)
  await next.close()
})

test('exceeding maxMemory in the allocator raises MemoryError and the pool recovers', async (ctx) => {
  skipIfWasm(ctx)
  await using pool = await Monty.create()
  const session = await pool.checkout({ limits: { maxMemory: 1024 } })
  // the fed snippet is memory the interpreter never accounts for — the worker
  // buys a frame buffer for it before it sees the code — so a snippet far
  // larger than the limit is caught by the allocator
  const error = await t.throwsAsync(() => session.feedRun('# ' + 'a'.repeat(16 * 1024 * 1024)), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'MemoryError: the worker exceeded its memory limit and was terminated')
  t.is(error.exception.typeName, 'MemoryError')
  const next = await pool.checkout()
  t.is(await next.feedRun('1 + 1'), 2)
  await next.close()
})

test('concurrent sessions run in distinct workers', async () => {
  await using pool = await Monty.create()
  const a = await pool.checkout()
  const b = await pool.checkout()
  try {
    t.not(a.workerId, b.workerId)
    const [ra, rb] = await Promise.all([a.feedRun('1 + 1'), b.feedRun('2 + 2')])
    t.is(ra, 2)
    t.is(rb, 4)
  } finally {
    await a.close()
    await b.close()
  }
})

test('exhausted pool times out the checkout', async () => {
  await using pool = await Monty.create({ maxProcesses: 1, checkoutTimeout: 0.2 })
  const held = await pool.checkout()
  try {
    const error = await t.throwsAsync(() => pool.checkout())
    t.is(error.message, 'no monty worker became available within the checkout timeout')
  } finally {
    await held.close()
  }
})

test('released worker is handed to a waiting checkout', async () => {
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
  skipIfWasm(ctx)
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
  skipIfWasm(ctx)
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
  skipIfWasm(ctx)
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

test('requestTimeout kills a wedged worker', async () => {
  await using pool = await Monty.create({ maxProcesses: 1, requestTimeout: 0.5 })
  const session = await pool.checkout()
  let hostResponsive = false
  const timer = setTimeout(() => {
    hostResponsive = true
  }, 50)
  const error = await t.throwsAsync(() => session.feedRun('while True:\n    pass'), {
    instanceOf: MontyCrashedError,
  })
  clearTimeout(timer)
  t.true(hostResponsive)
  t.true(error.timedOut)
  t.is(error.message, 'RuntimeError: monty worker killed after exceeding request timeout of 500ms')
  await t.throwsAsync(() => session.feedRun('1'), { instanceOf: MontyCrashedError })
  await using fresh = await pool.checkout()
  t.is(await fresh.feedRun('6 * 7'), 42)
  await session.close()
})

// Mount I/O runs on the host side of the pool, so reading a FIFO must fail
// fast (a real read would block the host with no watchdog able to rescue it).
// The sandbox sees a catchable PermissionError and the session stays usable.
// Unix-only (mkfifo).
test('special files in mounts are rejected without blocking', async (ctx) => {
  skipIfWasm(ctx)
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
          mount: new MountDir({ hostPath: dir, virtualPath: '/mnt', mode: 'read-only' }),
        }),
      { instanceOf: MontyRuntimeError },
    )
    t.is(error.message, "PermissionError: [Errno 13] Permission denied: '/mnt/pipe'")
    await session.close()
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

test('suspension time does not consume the duration budget', async () => {
  // maxFeedDurationSecs measures sandbox execution time; the worker reports
  // it on every turn and its clock is paused while suspended. The host taking
  // twice the entire budget to answer an external call must therefore not
  // time the feed out.
  await using pool = await Monty.create()
  await using session = await pool.checkout({ limits: { maxFeedDurationSecs: 0.3 } })
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
test('worker environment is empty', async (ctx) => {
  skipIfWasm(ctx)
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
