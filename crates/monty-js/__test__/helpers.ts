// Shared test scaffolding: one worker pool per spec file, with a `run` helper
// executing one snippet in a fresh session — the moral equivalent of
// pydantic_monty's `monty_run` fixture.

import { afterAll as afterEachFile, beforeAll as beforeEachFile } from 'vitest'
import { kind } from './env.js'
import { Monty, type CheckoutOptions, type FeedOptions } from '@pydantic/monty'

/** Checkout-level and feed-level options, flattened for convenience. */
export interface RunOptions extends FeedOptions, CheckoutOptions {}

export interface PoolFixture {
  /** Runs one snippet in a fresh session and returns its result. */
  run: (code: string, options?: RunOptions) => Promise<unknown>
  /** The shared pool, for tests that manage sessions directly. */
  pool: () => Monty
}

/**
 * Registers before/after hooks creating and closing the spec file's shared
 * pool, and returns the `run` helper bound to it.
 */
export function setupPool(): PoolFixture {
  let pool: Monty | null = null
  beforeEachFile(async () => {
    pool = await Monty.create(kind === 'browser' ? { maxCheckoutsPerWorker: 1 } : {})
  })
  afterEachFile(async () => {
    await pool?.close()
  })
  const get = () => {
    if (pool === null) {
      throw new Error('pool not started')
    }
    return pool
  }
  const run = async (code: string, options: RunOptions = {}) => {
    const { scriptName, limits, typeCheck, typeCheckStubs, assertMessageAnnotations, ...feed } = options
    const session = await get().checkout({
      ...(scriptName !== undefined ? { scriptName } : {}),
      ...(limits !== undefined ? { limits } : {}),
      ...(typeCheck !== undefined ? { typeCheck } : {}),
      ...(typeCheckStubs !== undefined ? { typeCheckStubs } : {}),
      ...(assertMessageAnnotations !== undefined ? { assertMessageAnnotations } : {}),
    })
    try {
      return await session.feedRun(code, feed)
    } finally {
      await session.close()
    }
  }
  return { run, pool: get }
}
