import test from 'ava'

import { MontyRuntimeError } from '../ts/index.js'
import { setupPool } from './helpers.js'

const { pool } = setupPool(test)

test('feed preserves state without replay', async (t) => {
  const session = await pool().checkout()
  try {
    await session.feedRun('counter = 0')
    t.is(await session.feedRun('counter = counter + 1'), null)
    t.is(await session.feedRun('counter'), 1)
    t.is(await session.feedRun('counter = counter + 1'), null)
    t.is(await session.feedRun('counter'), 2)
  } finally {
    await session.close()
  }
})

test('runtime error does not kill the session', async (t) => {
  const session = await pool().checkout()
  try {
    await session.feedRun('x = 1')
    const error = await t.throwsAsync(() => session.feedRun('1 / 0'), { instanceOf: MontyRuntimeError })
    t.is(error.message, 'ZeroDivisionError: division by zero')
    t.is(
      error.display(),
      [
        'Traceback (most recent call last):',
        '  File "<python-input-1>", line 1, in <module>',
        '    1 / 0',
        '    ~~~~~',
        'ZeroDivisionError: division by zero',
      ].join('\n'),
    )
    // Earlier globals survive the failed feed.
    t.is(await session.feedRun('x'), 1)
  } finally {
    await session.close()
  }
})

test('session dump returns opaque state', async (t) => {
  const session = await pool().checkout()
  try {
    await session.feedRun('x = 40')
    t.is(await session.feedRun('x = x + 1'), null)
    const state = await session.dump()
    t.true(Buffer.isBuffer(state))
    t.true(state.length > 0)
    // Dumping does not disturb the live session.
    t.is(await session.feedRun('x + 1'), 42)
  } finally {
    await session.close()
  }
})
