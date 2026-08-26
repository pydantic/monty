// The wasm path decodes values in TypeScript (`ts/worker/value.ts`) rather than
// through napi, so its codec is a second, independent implementation of the
// `monty-proto` wire format. `datetime.time` exercises the parts most easily got
// wrong: implicit-presence scalars that vanish at zero, explicit-presence
// `offsetSeconds`/`timezoneName`, and `fold` sitting after them at field 7.

import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'
import { Monty } from '@pydantic/monty/wasm'

test('a time decodes over the wasm transport', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create()
  await using session = await pool.checkout({})

  // every field at its default: only the submessage key reaches the wire
  t.deepEqual(await session.feedRun('import datetime\ndatetime.time(0, 0)'), {
    __monty_type__: 'Time',
    hour: 0,
    minute: 0,
    second: 0,
    microsecond: 0,
    fold: 0,
  })

  const aware = 'import datetime\ndatetime.time(23, 59, 59, 999999, datetime.timezone(datetime.timedelta(hours=-5)))'
  t.deepEqual(await session.feedRun(aware), {
    __monty_type__: 'Time',
    hour: 23,
    minute: 59,
    second: 59,
    microsecond: 999999,
    offsetSeconds: -18000,
    fold: 0,
  })
})

test('a time round-trips through the wasm transport', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create()
  await using session = await pool.checkout({})

  const time = {
    __monty_type__: 'Time',
    hour: 1,
    minute: 2,
    second: 3,
    microsecond: 4,
    offsetSeconds: 7200,
    timezoneName: 'P2',
    fold: 1,
  }
  t.deepEqual(await session.feedRun('x', { inputs: { x: time } }), time)
  t.is(await session.feedRun('x.isoformat()', { inputs: { x: time } }), '01:02:03.000004+02:00')

  // A zero offset is what explicit presence is for: it is the only thing making
  // this time aware, so encoding it away as a proto3 default would hand back a
  // naive time that formats without a suffix.
  const utc = {
    __monty_type__: 'Time',
    hour: 12,
    minute: 0,
    second: 0,
    microsecond: 0,
    offsetSeconds: 0,
    fold: 0,
  }
  t.deepEqual(await session.feedRun('x', { inputs: { x: utc } }), utc)
  t.is(await session.feedRun('x.isoformat()', { inputs: { x: utc } }), '12:00:00+00:00')
})
