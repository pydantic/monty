import { test } from 'vitest'

import { runSmokeTest } from './test.js'

test('released package runs in a browser worker', async () => {
  await runSmokeTest({ expectWorkerPid: false })
})
