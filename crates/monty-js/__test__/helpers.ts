// Shared test scaffolding: one worker pool per spec file, with a `run` helper
// executing one snippet in a fresh session — the moral equivalent of
// pydantic_monty's `monty_run` fixture.

import { afterAll as afterEachFile, beforeAll as beforeEachFile } from 'vitest'
import { kind } from './env.js'
import { Monty, type CheckoutOptions, type FeedOptions } from '@pydantic/monty'
import { t } from './assertions.js'

/** Checkout-level and feed-level options, flattened for convenience. */
export interface RunOptions extends FeedOptions, CheckoutOptions {}

export interface PoolFixture {
  /** Runs one snippet in a fresh session and returns its result. */
  run: (code: string, options?: RunOptions) => Promise<unknown>
  /** The shared pool, for tests that manage sessions directly. */
  pool: () => Monty
}

/** Checks path rejection and error spelling through native and WASM OS callbacks. */
export async function checkOsPathValidation(run: PoolFixture['run']): Promise<void> {
  const calls: unknown[] = []
  const result = await run(
    `import os
from pathlib import Path
errors = []
for operation in [
    lambda: open(path),
    lambda: Path(path).read_text(),
    lambda: os.chdir(path),
    lambda: os.rename(path, 'dst'),
    lambda: os.rename('src', path),
]:
    try:
        operation()
    except ValueError as e:
        errors.append(str(e))
p = Path(path)
(errors, p.exists(), p.is_file(), p.is_dir(), p.is_symlink(), os.getcwd())`,
    {
      cwd: '/data',
      inputs: { path: 'bad\0/../x' },
      os: (...args) => {
        calls.push(args)
        return true
      },
    },
  )
  t.deepEqual(result, [
    [
      'embedded null byte',
      'embedded null byte',
      'stat: embedded null character in path',
      'rename: embedded null character in src',
      'rename: embedded null character in dst',
    ],
    false,
    false,
    false,
    false,
    '/data',
  ])
  t.deepEqual(calls, [])
  for (const [code, expected] of [
    ['import os\nos.listdir()', "PermissionError: Permission denied: '/'"],
    ["open('./x')", "PermissionError: Permission denied: '/x'"],
    ["open('')", "PermissionError: Permission denied: ''"],
    ["open('bad\\0/../x')", 'ValueError: embedded null byte'],
  ] as const) {
    const error = await t.throwsAsync(() => run(code, { cwd: '/' }))
    t.is(error.message, expected)
  }
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
    const {
      scriptName,
      limits,
      typeCheck,
      typeCheckStubs,
      typeCheckFormat,
      typeCheckColor,
      assertMessageAnnotations,
      printFlushInterval,
      ...feed
    } = options
    const session = await get().checkout({
      ...(scriptName !== undefined ? { scriptName } : {}),
      ...(limits !== undefined ? { limits } : {}),
      ...(typeCheck !== undefined ? { typeCheck } : {}),
      ...(typeCheckStubs !== undefined ? { typeCheckStubs } : {}),
      ...(typeCheckFormat !== undefined ? { typeCheckFormat } : {}),
      ...(typeCheckColor !== undefined ? { typeCheckColor } : {}),
      ...(assertMessageAnnotations !== undefined ? { assertMessageAnnotations } : {}),
      ...(printFlushInterval !== undefined ? { printFlushInterval } : {}),
    })
    try {
      return await session.feedRun(code, feed)
    } finally {
      await session.close()
    }
  }
  return { run, pool: get }
}
