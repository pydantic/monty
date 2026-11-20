import { test } from 'vitest'

import { _defaultMaxCheckoutsPerWorker } from '../native-addon.js'
import { DEFAULT_MAX_CHECKOUTS_PER_WORKER } from '../ts/options.js'
import { t } from './assertions.js'

import { Monty, MontyConfigurationError } from '@pydantic/monty'

// The TypeScript copy serves the WASM pool too, which cannot load the addon.
test('the JS recycle default matches monty-pool', () => {
  t.is(DEFAULT_MAX_CHECKOUTS_PER_WORKER, _defaultMaxCheckoutsPerWorker())
})

// the background refill itself is covered by monty-pool's tests
test('preWarm keeps spare subprocess workers', async () => {
  await using pool = await Monty.create({ preWarm: 2, maxProcesses: 3 })
  await using first = await pool.checkout()
  await using second = await pool.checkout()
  t.is(await first.feedRun('1 + 1'), 2)
  t.is(await second.feedRun('2 + 2'), 4)
  t.not(first.workerPid, second.workerPid)
})

test('preWarm above maxProcesses rejects create', async () => {
  const error = await t.throwsAsync(() => Monty.create({ preWarm: 3, maxProcesses: 2 }), {
    instanceOf: MontyConfigurationError,
  })
  t.is(error.message, 'invalid pool size: pre_warm=3 max_processes=2')
})
