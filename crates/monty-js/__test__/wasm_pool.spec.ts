import { test } from 'vitest'

import { createWorkerPool, loadModule, type Monty, MontyCrashedError, MontyRuntimeError } from '@pydantic/monty/wasm'
import { t } from './assertions.js'

// The lower-level public entry must select the same backend as Monty.create().
test('precompiled modules run in a worker-backed pool', async () => {
  await using pool: Monty = await createWorkerPool(await loadModule(), { maxProcesses: 1 })
  await using session = await pool.checkout()
  t.is(await session.feedRun('6 * 7'), 42)
})

test('wasm custom exception type name is preserved in ExceptionInfo and display', async () => {
  await using pool: Monty = await createWorkerPool(await loadModule(), { maxProcesses: 1 })
  await using session = await pool.checkout()
  const code = `
class MyCustomError(ValueError):
    pass

raise MyCustomError('custom failure')
`
  const error = await t.throwsAsync(() => session.feedRun(code), { instanceOf: MontyRuntimeError })
  t.is(error.exception.typeName, 'ValueError')
  t.is(error.exception.customTypeName, 'MyCustomError')
  t.is(error.customTypeName, 'MyCustomError')
  t.is(error.display('type-msg'), 'MyCustomError: custom failure')
  t.is(error.display('msg'), 'custom failure')
  t.true(error.display('traceback').includes('MyCustomError: custom failure'))
})

test.each(['requestTimeout', 'checkoutTimeout', 'feedDurationLimitGrace', 'turnDurationLimitGrace'] as const)(
  'direct pool creation rejects invalid %s asynchronously',
  async (option) => {
    const promise = createWorkerPool({}, { minProcesses: 0, [option]: -1 })
    t.true(promise instanceof Promise)
    const error = await t.throwsAsync(() => promise)
    t.is(error.message, `${option} must be a finite non-negative number`)
  },
)

test('component initialization failures reject pool creation', async () => {
  const error = await t.throwsAsync(() => createWorkerPool({}), { instanceOf: MontyCrashedError })
  t.false(error.timedOut)
  t.is(
    error.message,
    'RuntimeError: worker initialization failed: component core module is missing: monty.component.core.wasm',
  )
})
