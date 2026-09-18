// The `checkout()` clock, sleep and `random` options through the wasm worker,
// driven in Node so it needs no browser: they cross the component's
// `configure` request rather than the subprocess wire.

import { test } from 'vitest'

import { Monty, type MontyDateTime } from '@pydantic/monty/wasm'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

test('a fixed clock, zero sleeps and a seed reach the wasm worker', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const session = await pool.checkout({
    autoOsCalls: {
      datetime: new Date('2024-01-15T10:30:05.123Z'),
      timezone: { offsetSeconds: 3600, name: 'CET' },
      sleep: 'zero',
      randomStart: { seed: 42 },
    },
  })
  try {
    const code =
      'import random, time\nfrom datetime import datetime\ntime.sleep(3600)\n(datetime.now(), random.random())'
    const [now, draw] = (await session.feedRun(code)) as [MontyDateTime, number]
    t.deepEqual(now, {
      __monty_type__: 'DateTime',
      year: 2024,
      month: 1,
      day: 15,
      hour: 11,
      minute: 30,
      second: 5,
      microsecond: 123000,
    })
    t.is(draw, 0.6394267984578837)
  } finally {
    await session.close()
    await pool.close()
  }
})
