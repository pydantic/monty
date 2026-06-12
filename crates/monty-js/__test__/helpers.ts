// Shared test scaffolding: one worker pool per spec file (ava runs each file
// in its own process), with a `run` helper executing one snippet in a fresh
// session — the moral equivalent of pydantic_monty's `monty_run` fixture.

import type { TestFn } from 'ava'
import { Monty, type CheckoutOptions, type FeedOptions } from '../ts/index.js'

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
export function setupPool(test: TestFn): PoolFixture {
  let pool: Monty | null = null
  test.before(async () => {
    pool = await Monty.create()
  })
  test.after.always(async () => {
    await pool?.close()
  })
  const get = () => {
    if (pool === null) {
      throw new Error('pool not started')
    }
    return pool
  }
  const run = async (code: string, options: RunOptions = {}) => {
    const { scriptName, limits, typeCheck, typeCheckStubs, ...feed } = options
    const session = await get().checkout({
      ...(scriptName !== undefined ? { scriptName } : {}),
      ...(limits !== undefined ? { limits } : {}),
      ...(typeCheck !== undefined ? { typeCheck } : {}),
      ...(typeCheckStubs !== undefined ? { typeCheckStubs } : {}),
    })
    try {
      return await session.feedRun(code, feed)
    } finally {
      await session.close()
    }
  }
  return { run, pool: get }
}
