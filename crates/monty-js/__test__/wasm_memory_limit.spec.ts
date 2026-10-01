import { test } from 'vitest'

import { Monty, MontyCrashedError } from '@pydantic/monty/wasm'
import { t } from './assertions.js'

test('a hard allocator limit traps the component and the pool recovers', async () => {
  await using pool = await Monty.create({ maxProcesses: 1 })
  await using session = await pool.checkout({ limits: { maxMemory: 1024 } })
  // Component argument allocation precedes interpreter checkpoints; a trap has no allocator-specific exit code.
  const error = await t.throwsAsync(() => session.feedRun('# ' + 'a'.repeat(16 * 1024 * 1024)), {
    instanceOf: MontyCrashedError,
  })
  t.is(error.message, 'RuntimeError: worker exited without a turn-ending event')
  await using next = await pool.checkout()
  t.is(await next.feedRun('3 + 3'), 6)
})
