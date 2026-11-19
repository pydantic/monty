import { test } from 'vitest'

import { _defaultMaxCheckoutsPerWorker } from '../native-addon.js'
import { DEFAULT_MAX_CHECKOUTS_PER_WORKER } from '../ts/options.js'
import { t } from './assertions.js'

// The TypeScript copy serves the WASM pool too, which cannot load the addon.
test('the JS recycle default matches monty-pool', () => {
  t.is(DEFAULT_MAX_CHECKOUTS_PER_WORKER, _defaultMaxCheckoutsPerWorker())
})
