import { expect, test } from 'vitest'

import { throwsAsync } from './assertions.js'

interface PoolOptions {
  minProcesses?: number
  maxProcesses?: number
  checkoutTimeout?: number
  requestTimeout?: number
  durationLimitGrace?: number | null
  maxCheckoutsPerWorker?: number
}

interface CheckoutOptions {
  limits?: { maxDurationSecs?: number }
}

interface Session {
  readonly workerId?: number
  feedRun(code: string, options?: FeedOptions): Promise<unknown>
  close(): Promise<void>
}

interface FeedOptions {
  externalLookup?: Record<string, (...args: never[]) => unknown>
}

interface Pool {
  checkout(options?: CheckoutOptions): Promise<Session>
  close(): Promise<void>
}

interface CrashError extends Error {
  timedOut: boolean
  exitStatus: string | null
}

interface PoolCapabilities {
  /** Whether timeout-killed workers have an environment exit status. */
  timeoutExitStatus: boolean
}

/** Registers lifecycle, timeout, recycling, and recovery tests for a pool backend. */
export function runPoolConformanceTests(
  name: string,
  create: (options?: PoolOptions) => Promise<Pool>,
  capabilities: PoolCapabilities,
): void {
  test(`${name}: checkout after close rejects`, async () => {
    const pool = await create()
    await pool.close()
    const error = await throwsAsync(() => pool.checkout())
    expect(error.message).toBe('the pool is closed — create a new Monty pool')
  })

  test(`${name}: close is idempotent`, async () => {
    const pool = await create()
    await Promise.all([pool.close(), pool.close()])
    await pool.close()
  })

  test(`${name}: feed after session close rejects`, async () => {
    await using pool = await create()
    const session = await pool.checkout()
    await session.close()
    const error = await throwsAsync(() => session.feedRun('1'))
    expect(error.message).toBe('the session is closed — check out a new one')
  })

  test(`${name}: workers are reused across checkouts`, async () => {
    await using pool = await create({ maxProcesses: 1 })
    const first = await pool.checkout()
    const workerId = first.workerId
    expect(workerId).toBeTypeOf('number')
    await first.close()
    const second = await pool.checkout()
    expect(second.workerId).toBe(workerId)
    await second.close()
  })

  test(`${name}: maxCheckoutsPerWorker recycles the worker`, async () => {
    await using pool = await create({ maxProcesses: 1, maxCheckoutsPerWorker: 1 })
    const first = await pool.checkout()
    const workerId = first.workerId
    await first.close()
    const second = await pool.checkout()
    expect(second.workerId).not.toBe(workerId)
    await second.close()
  })

  test(`${name}: concurrent sessions use distinct workers`, async () => {
    await using pool = await create({ maxProcesses: 2 })
    const a = await pool.checkout()
    const b = await pool.checkout()
    try {
      expect(a.workerId).not.toBe(b.workerId)
      await expect(Promise.all([a.feedRun('1 + 1'), b.feedRun('2 + 2')])).resolves.toEqual([2, 4])
    } finally {
      await a.close()
      await b.close()
    }
  })

  test(`${name}: exhausted checkout observes checkoutTimeout`, async () => {
    await using pool = await create({ maxProcesses: 1, checkoutTimeout: 0.05 })
    const held = await pool.checkout()
    try {
      const error = await throwsAsync(() => pool.checkout())
      expect(error.message).toBe('no monty worker became available within the checkout timeout')
    } finally {
      await held.close()
    }
  })

  test(`${name}: released workers are handed to waiting checkouts`, async () => {
    await using pool = await create({ maxProcesses: 1 })
    const held = await pool.checkout()
    const waiting = pool.checkout()
    await held.close()
    const session = await waiting
    await expect(session.feedRun('40 + 2')).resolves.toBe(42)
    await session.close()
  })

  test(`${name}: requestTimeout preserves crash metadata and the pool recovers`, async () => {
    await using pool = await create({ maxProcesses: 1, checkoutTimeout: 1, requestTimeout: 0.2 })
    const crashed = await pool.checkout()
    const crash = await throwsAsync<CrashError>(() => crashed.feedRun('while True:\n    pass'))
    expect(crash.name).toBe('MontyCrashedError')
    expect(crash.timedOut).toBe(true)
    expect(crash.message).toBe('RuntimeError: monty worker killed after exceeding request timeout of 200ms')
    if (capabilities.timeoutExitStatus) expect(crash.exitStatus).toMatch(/^exit code: /)
    else expect(crash.exitStatus).toBeNull()

    // Capacity is released as soon as the worker dies, not when its poisoned
    // session is eventually closed.
    const fresh = await pool.checkout()
    await expect(fresh.feedRun('1 + 1')).resolves.toBe(2)
    await fresh.close()
    await crashed.close()
  })

  test(`${name}: suspension time does not consume maxDurationSecs`, async () => {
    await using pool = await create()
    await using session = await pool.checkout({ limits: { maxDurationSecs: 0.2 } })
    const result = await session.feedRun("await fetch_data('u') + '!'", {
      externalLookup: {
        fetch_data: async () => {
          await new Promise((resolve) => setTimeout(resolve, 400))
          return 'body'
        },
      },
    })
    expect(result).toBe('body!')
  })
}
