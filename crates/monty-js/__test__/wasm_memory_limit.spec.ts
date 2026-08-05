// The wasm worker's memory limit, driven in Node so it needs no browser.
//
// The module declares the same `monty-alloc` global allocator as the subprocess
// worker. Soft overruns raise `MemoryError`; a trapped module has no exit
// status, so only an overrun of the higher hard limit surfaces as a crash.

import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

// everything comes from `/wasm`, which re-exports the error classes: importing
// them from `@pydantic/monty` would pull in the napi loader, and this suite
// runs where only the wasm module has been built
import { Monty, MontyCrashedError, MontyRuntimeError } from '@pydantic/monty/wasm'

test('a session limit leaves normal wasm work alone', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const session = await pool.checkout({ limits: { maxMemory: 1024 * 1024 } })
  t.is(await session.feedRun('sum(range(1000))'), 499500)
  await session.close()
  await pool.close()
})

test('an overrun the interpreter catches leaves the instance alive', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const session = await pool.checkout({ limits: { maxMemory: 1024 * 1024 } })
  // The incomplete comprehension is unwound at a checkpoint before the hard limit.
  const error = await t.throwsAsync(() => session.feedRun('[str(i) for i in range(131_072)]'), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'MemoryError: memory limit exceeded: 1048600 bytes > 1048576 bytes')
  t.is(error.exception.typeName, 'MemoryError')
  t.is(await session.feedRun('1 + 1'), 2)
  await session.close()
  await pool.close()
})

test('exceeding the limit kills the instance and the pool recovers', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const session = await pool.checkout({ limits: { maxMemory: 1024 } })
  // The module allocates the frame buffer before the interpreter can reach a
  // soft-limit checkpoint.
  const error = await t.throwsAsync(() => session.feedRun('# ' + 'a'.repeat(16 * 1024 * 1024)), {
    instanceOf: MontyCrashedError,
  })
  // the trap, not a classified MemoryError: a wasm module has no exit status
  t.is(error.message, 'RuntimeError: worker exited without a turn-ending event')
  const next = await pool.checkout()
  t.is(await next.feedRun('3 + 3'), 6)
  await next.close()
  await pool.close()
})
