import { test } from 'vitest'

import { createWorkerPool, loadModule, type Monty, MontyCrashedError } from '@pydantic/monty/wasm'
import { t } from './assertions.js'

// The lower-level public entry must select the same backend as Monty.create().
test('precompiled modules run in a worker-backed pool', async () => {
  await using pool: Monty = await createWorkerPool(await loadModule(), { maxProcesses: 1, requestTimeout: 0.5 })
  await using session = await pool.checkout()
  t.is(await session.feedRun('6 * 7'), 42)
})

test('component initialization failures reject pool creation', async () => {
  const error = await t.throwsAsync(() => createWorkerPool({}), { instanceOf: MontyCrashedError })
  t.false(error.timedOut)
  t.is(
    error.message,
    'RuntimeError: worker initialization failed: component core module is missing: monty.component.core.wasm',
  )
})
