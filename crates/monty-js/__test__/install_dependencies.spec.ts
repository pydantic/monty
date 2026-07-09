import { test } from 'vitest'
import { t } from './assertions.js'

import { Monty, MontyRuntimeError } from '../ts/index.js'

// These run against the default `monty` *sandbox* worker, which has no host
// interpreter to install for and rejects the request. A real install
// end-to-end is covered by the `monty-cpython` Rust tests.

const isRuntimeError = { instanceOf: MontyRuntimeError }

test('installDependencies is rejected by the sandbox worker, session survives', async () => {
  await using pool = await Monty.create()
  const session = await pool.checkout()
  const error = await t.throwsAsync(() => session.installDependencies(['httpx>=0.27']), isRuntimeError)
  t.is(error.message, 'RuntimeError: dependency installation is only supported by the CPython worker')
  // the session survives the rejection
  t.is(await session.feedRun('1 + 1'), 2)
})

test('installDependencies with an empty list is a no-op', async () => {
  await using pool = await Monty.create()
  const session = await pool.checkout()
  t.is(await session.installDependencies([]), undefined)
  t.is(await session.feedRun('1 + 1'), 2)
})
