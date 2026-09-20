// Exercises checkout options through the component's `configure` request in Node.

import { test } from 'vitest'

import { Monty, MontyRuntimeError, type MontyDateTime } from '@pydantic/monty/wasm'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

test('the wasm pool charges system sleeps to maxTotalSleepSecs', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const session = await pool.checkout({ limits: { maxTotalSleepSecs: 0.25 } })
  try {
    // exact binary fractions, so the reported total is exact too
    const code = 'import time\ntime.sleep(0.125)\ntry:\n    time.sleep(0.5)\nexcept TimeoutError:\n    pass\n'
    const error = await t.throwsAsync(() => session.feedRun(code), { instanceOf: MontyRuntimeError })
    t.is(error.exception.typeName, 'TimeoutError')
    t.is(error.display('msg'), 'sleep limit exceeded: 625ms > 250ms')
  } finally {
    await session.close()
    await pool.close()
  }
})

test('the wasm pool passes processTime through to the component', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const hidden = await pool.checkout()
  const elapsed = await pool.checkout({ autoOsCalls: { processTime: 'elapsed' } })
  try {
    const code = 'import time\nfor _ in range(200000):\n    pass\ntime.process_time() > 0.0'
    t.is(await hidden.feedRun(code), false)
    t.is(await elapsed.feedRun(code), true)
  } finally {
    await hidden.close()
    await elapsed.close()
    await pool.close()
  }
})

test('the wasm pool keeps its sleep limit as a ceiling across a load', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const source = await pool.checkout()
  const kept = await pool.checkout({ limits: { maxTotalSleepSecs: 0.25 } })
  try {
    // a dump made without a limit does not loosen the checkout's
    await kept.loadSession(await source.dump())
    t.is(await kept.feedRun('import time\ntime.sleep(0.125)'), null)
    const error = await t.throwsAsync(() => kept.feedRun('import time\ntime.sleep(0.5)'), {
      instanceOf: MontyRuntimeError,
    })
    t.is(error.display('msg'), 'sleep limit exceeded: 625ms > 250ms')
    // and a dump's limit tightens a checkout that set none
    const capped = await pool.checkout({ limits: { maxTotalSleepSecs: 0.25 } })
    const adopted = await pool.checkout()
    try {
      await adopted.loadSession(await capped.dump())
      const refused = await t.throwsAsync(() => adopted.feedRun('import time\ntime.sleep(0.5)'), {
        instanceOf: MontyRuntimeError,
      })
      t.is(refused.display('msg'), 'sleep limit exceeded: 500ms > 250ms')
    } finally {
      await adopted.close()
      await capped.close()
    }
  } finally {
    await source.close()
    await kept.close()
    await pool.close()
  }
})

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

test('a named zone resolves from the tz database bundled into the wasm worker', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const session = await pool.checkout({
    autoOsCalls: { datetime: new Date('2024-06-15T12:30:00Z'), timezone: 'Europe/London' },
  })
  try {
    const code =
      'import time\nfrom datetime import datetime\n(datetime.now().hour, datetime.now().astimezone().tzname(), time.tzname)'
    t.deepEqual(await session.feedRun(code), [13, 'BST', ['GMT', 'BST']])
  } finally {
    await session.close()
    await pool.close()
  }
})
