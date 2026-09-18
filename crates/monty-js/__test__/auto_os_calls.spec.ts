// The `checkout()` options choosing which OS calls the sandbox answers itself:
// the clock (`datetime`), the sleeps (`sleep`, `sandboxSleepClamp`) and where
// `random` starts (`randomStart`).

import { test } from 'vitest'
import { t } from './assertions.js'
import type { MontyDate, MontyDateTime } from '@pydantic/monty'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

// =============================================================================
// datetime
// =============================================================================

test('the worker clock answers by default, with no os callback', async () => {
  const before = Date.now() / 1000 - 60
  const now = (await run('import time\ntime.time()')) as number
  t.true(now >= before && now <= Date.now() / 1000 + 60)
})

test('a Date freezes the clock at that instant, read as UTC', async () => {
  const frozen = new Date('2024-01-15T10:30:05.123Z')
  const code = [
    'import time',
    'from datetime import date, datetime, timezone',
    '(datetime.now(), date.today(), time.time(), datetime.now(timezone.utc), datetime.now() == datetime.now())',
  ].join('\n')
  const [naive, today, epoch, aware, frozenAgain] = (await run(code, { datetime: frozen })) as [
    MontyDateTime,
    MontyDate,
    number,
    MontyDateTime,
    boolean,
  ]
  const wall = { year: 2024, month: 1, day: 15, hour: 10, minute: 30, second: 5, microsecond: 123000 }
  t.deepEqual(naive, { __monty_type__: 'DateTime', ...wall })
  t.deepEqual(today, { __monty_type__: 'Date', year: 2024, month: 1, day: 15 })
  t.is(epoch, 1705314605.123)
  t.deepEqual(aware, { __monty_type__: 'DateTime', ...wall, offsetSeconds: 0 })
  t.true(frozenAgain)
})

test('call_host sends the clock to the os callback', async () => {
  const calls: unknown[] = []
  const result = await run('import time\ntime.time()', {
    datetime: 'call_host',
    os: (name, args) => {
      calls.push([name, args])
      return 7.5
    },
  })
  t.is(result, 7.5)
  t.deepEqual(calls, [['time.time', []]])
})

test('an invalid datetime is rejected before the checkout', async () => {
  await t.throwsAsync(() => pool().checkout({ datetime: new Date('nope') }), {
    instanceOf: RangeError,
    message: "datetime must be 'system', 'call_host' or a valid Date",
  })
  await t.throwsAsync(() => pool().checkout({ datetime: 'later' as 'system' }), { instanceOf: RangeError })
})

// =============================================================================
// sleep
// =============================================================================

test('zero returns at once', async () => {
  const started = performance.now()
  const code = "import asyncio, time\ntime.sleep(3600)\nasyncio.run(asyncio.sleep(3600, 'woken'))"
  t.is(await run(code, { sleep: 'zero' }), 'woken')
  t.true(performance.now() - started < 5000)
})

test('the clamp cuts a sandbox sleep short', async () => {
  const started = performance.now()
  const code = "import asyncio, time\ntime.sleep(3600)\nasyncio.run(asyncio.sleep(3600, 'woken'))"
  t.is(await run(code, { sandboxSleepClamp: 0.001 }), 'woken')
  t.true(performance.now() - started < 5000)
  t.is(await run('import time\ntime.sleep(0.001)', { sandboxSleepClamp: Infinity }), null)
})

test('gathered sandbox sleeps overlap', async () => {
  const code = [
    'import asyncio, time',
    'async def w(n):',
    '    await asyncio.sleep(0.05, n)',
    '    return n * 2',
    'async def main():',
    '    return await asyncio.gather(w(1), w(2), w(3))',
    't = time.time()',
    'r = asyncio.run(main())',
    '(r, time.time() - t < 0.14)',
  ].join('\n')
  t.deepEqual(await run(code), [[2, 4, 6], true])
})

test('invalid sleep options are rejected before the checkout', async () => {
  await t.throwsAsync(() => pool().checkout({ sleep: 'forever' as 'zero' }), { instanceOf: RangeError })
  await t.throwsAsync(() => pool().checkout({ sandboxSleepClamp: -1 }), {
    instanceOf: RangeError,
    message: 'sandboxSleepClamp must be a non-negative number of seconds (Infinity for no cap)',
  })
  await t.throwsAsync(() => pool().checkout({ sandboxSleepClamp: NaN }), { instanceOf: RangeError })
})

// =============================================================================
// randomStart
// =============================================================================

// CPython: random.seed(s); random.random(), random.randint(1, 100)
const SEEDS: [number | bigint | string | Uint8Array, [number, number]][] = [
  [42, [0.6394267984578837, 4]],
  [-42, [0.6394267984578837, 4]],
  [2n ** 70n, [0.2327882718301838, 54]],
  [1.5, [0.551763726942059, 33]],
  ['abc', [0.7720246314157545, 72]],
  [new TextEncoder().encode('abc'), [0.7720246314157545, 72]],
]

test('a seed starts random exactly as random.seed would', async () => {
  for (const [seed, expected] of SEEDS) {
    const code = 'import random\n[random.random(), random.randint(1, 100)]'
    t.deepEqual(await run(code, { randomStart: { seed } }), expected, String(seed))
  }
})

test('a seed persists across feeds and random.seed still wins', async () => {
  const session = await pool().checkout({ randomStart: { seed: 42 } })
  try {
    await session.feedRun('import random')
    t.is(await session.feedRun('random.random()'), 0.6394267984578837)
    await session.feedRun('random.seed(5)')
    t.is(await session.feedRun('random.random()'), 0.6229016948897019)
  } finally {
    await session.close()
  }
})

test('unseeded instances under a seed are deterministic and distinct', async () => {
  const code = 'import random\n[random.Random().random(), random.Random().random(), random.random()]'
  const first = (await run(code, { randomStart: { seed: 42 } })) as number[]
  const second = await run(code, { randomStart: { seed: 42 } })
  t.deepEqual(first, second)
  t.is(new Set(first).size, 3)
  t.is(first[2], 0.6394267984578837)
})

test('an invalid randomStart is rejected before the checkout', async () => {
  await t.throwsAsync(() => pool().checkout({ randomStart: { seed: true as unknown as number } }), {
    instanceOf: TypeError,
    message: "randomStart must be 'random' or { seed: number | bigint | string | Uint8Array }",
  })
  await t.throwsAsync(() => pool().checkout({ randomStart: { sead: 1 } as unknown as 'random' }), {
    instanceOf: TypeError,
  })
})
