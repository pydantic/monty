import { test } from 'vitest'
import { t } from './assertions.js'

import {
  FutureSnapshot,
  MontyComplete,
  MontyRuntimeError,
  ProtocolError,
  type FeedOptions,
  type MontySession,
} from '@pydantic/monty'
import { setupPool } from './helpers.js'
import { isWasm } from './env.js'

const { pool } = setupPool()

test('feed preserves state without replay', async () => {
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

test('runtime error does not kill the session', async () => {
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

test('session dump returns opaque state', async () => {
  const session = await pool().checkout()
  try {
    await session.feedRun('x = 40')
    t.is(await session.feedRun('x = x + 1'), null)
    const state = await session.dump()
    t.true(state instanceof Uint8Array)
    t.true(state.length > 0)
    // Dumping does not disturb the live session.
    t.is(await session.feedRun('x + 1'), 42)
  } finally {
    await session.close()
  }
})

test.each([
  ['run', 'run'],
  ['run', 'manual'],
  ['manual', 'run'],
  ['manual', 'manual'],
] as const)('external future survives %s to %s feeds', async (first, second) => {
  const pending = deferred<number>()
  const session = await pool().checkout()
  try {
    await feed(session, first, 'saved = later()', { externalLookup: { later: () => pending.promise } })
    pending.resolve(42)
    t.is(await feed(session, second, 'await saved'), 42)
    t.is(await session.feedRun('await saved'), 42)
  } finally {
    pending.resolve(0)
    await session.close()
  }
})

test.each(['external', 'os', 'system'] as const)(
  'an unfinished %s future does not break an unrelated await in the next feed',
  async (kind) => {
    const pending = deferred<number>()
    const session = await pool().checkout(kind === 'os' ? { osPolicy: { sleep: 'call_host' } } : {})
    try {
      await session.feedRun(kind === 'external' ? 'later()\nNone' : 'import asyncio\nasyncio.sleep(0.01)\nNone', {
        externalLookup: { later: () => pending.promise },
        os: () => pending.promise,
      })
      t.is(await session.feedRun('await fresh()', { externalLookup: { fresh: async () => 42 } }), 42)
    } finally {
      pending.resolve(0)
      await session.close()
    }
  },
)

test('later feeds use their own handlers while retaining earlier results', async () => {
  const pending = deferred<number>()
  const session = await pool().checkout({ osPolicy: { sleep: 'call_host' } })
  const calls: string[] = []
  try {
    await session.feedRun('saved = later()\nimport asyncio', {
      externalLookup: { later: () => pending.promise },
      os: () => {
        throw new Error('old OS handler must not be reused')
      },
    })
    const error = await t.throwsAsync(() => session.feedRun('later()'), { instanceOf: MontyRuntimeError })
    t.is(error.message, "NameError: name 'later' is not defined")
    pending.resolve(40)
    t.deepEqual(
      await session.feedRun('[await saved, later(), await asyncio.sleep(0)]', {
        externalLookup: { later: () => 2 },
        os: async (name) => {
          calls.push(name)
          return null
        },
      }),
      [40, 2, null],
    )
    t.deepEqual(calls, ['asyncio.sleep'])
  } finally {
    pending.resolve(0)
    await session.close()
  }
})

test('a rejected future remains catchable in a later feed', async () => {
  const pending = deferred<number>()
  const session = await pool().checkout()
  try {
    await session.feedRun('saved = later()', { externalLookup: { later: () => pending.promise } })
    pending.reject(Object.assign(new Error('callback failed'), { name: 'ValueError' }))
    t.is(
      await session.feedRun('try:\n    await saved\nexcept ValueError as e:\n    message = str(e)\nmessage'),
      'callback failed',
    )
    t.is(await session.feedRun('1 + 1'), 2)
  } finally {
    pending.resolve(0)
    await session.close()
  }
})

test('pending futures stay isolated between sessions', async () => {
  const a = deferred<number>()
  const b = deferred<number>()
  const first = await pool().checkout()
  const second = await pool().checkout()
  try {
    await first.feedRun('saved = later()', { externalLookup: { later: () => a.promise } })
    await second.feedRun('saved = later()', { externalLookup: { later: () => b.promise } })
    b.resolve(22)
    t.is(await second.feedRun('await saved'), 22)
    a.resolve(11)
    t.is(await first.feedRun('await saved'), 11)
  } finally {
    a.resolve(0)
    b.resolve(0)
    await first.close()
    await second.close()
  }
})

test('manually delivered futures are retired before their host promise settles', async () => {
  const pending = deferred<number>()
  const session = await pool().checkout()
  try {
    await session.feedRun('saved = later()', { externalLookup: { later: () => pending.promise } })
    const snapshot = await session.feedStart('await saved')
    if (!(snapshot instanceof FutureSnapshot)) throw new Error('expected pending future')
    const done = await snapshot.resume([{ callId: snapshot.pendingCallIds[0], value: 42 }])
    t.true(done instanceof MontyComplete)
    t.is((done as MontyComplete).output, 42)
    const tracked = Reflect.get(session, 'futures') as Map<number, unknown>
    t.is(tracked.size, 0)
    pending.resolve(99)
    await pending.promise
    t.is(tracked.size, 0)
    t.is(await session.feedRun('await saved'), 42)
  } finally {
    pending.resolve(0)
    await session.close()
  }
})

test.each(['close', 'lookup', 'print'] as const)(
  'session %s releases future tracking before late rejection',
  async (exit) => {
    const pending = deferred<number>()
    const session = await pool().checkout()
    try {
      await session.feedRun('saved = later()', { externalLookup: { later: () => pending.promise } })
      const tracked = Reflect.get(session, 'futures') as Map<number, unknown>
      t.is(tracked.size, 1)
      if (exit === 'close') {
        await session.close()
      } else if (exit === 'lookup') {
        await t.throwsAsync(() => session.feedRun('bad', { externalLookup: { bad: Symbol() } }))
      } else {
        await t.throwsAsync(
          () =>
            session.feedRun('print(1)\nother()', {
              printCallback: () => {
                throw new Error('stop printing')
              },
            }),
          { message: 'stop printing' },
        )
      }
      t.is(tracked.size, 0)
      pending.reject(new Error('settled after teardown'))
      await pending.promise.catch(() => {})
      t.is(tracked.size, 0)
    } finally {
      pending.resolve(0)
      await session.close()
    }
  },
)

test('invalid feeds release futures only when the session is lost', async () => {
  const pending = deferred<number>()
  const session = await pool().checkout()
  try {
    await session.feedRun('saved = later()', { externalLookup: { later: () => pending.promise } })
    const snapshot = await session.feedStart('await saved')
    const tracked = Reflect.get(session, 'futures') as Map<number, unknown>
    if (!(snapshot instanceof FutureSnapshot)) throw new Error('expected pending future')
    if (isWasm) {
      // The component rejects an invalid feed without discarding its suspension.
      await t.throwsAsync(() => session.feedRun('1'), { instanceOf: MontyRuntimeError })
      t.is(tracked.size, 1)
      pending.resolve(42)
      const done = await snapshot.resumeAuto()
      t.true(done instanceof MontyComplete)
      t.is((done as MontyComplete).output, 42)
    } else {
      await t.throwsAsync(() => session.feedRun('1'), { instanceOf: ProtocolError })
    }
    t.is(tracked.size, 0)
  } finally {
    pending.resolve(0)
    await session.close()
  }
})

test('idle dumps preserve future ids but not host promises', async () => {
  const pending = deferred<number>()
  const original = await pool().checkout()
  let state: Uint8Array
  try {
    await original.feedRun('saved = later()', { externalLookup: { later: () => pending.promise } })
    state = await original.dump()
  } finally {
    pending.resolve(99)
    await original.close()
  }
  const restored = await pool().checkout()
  try {
    await restored.loadSession(state)
    const snapshot = await restored.feedStart('await saved')
    if (!(snapshot instanceof FutureSnapshot)) throw new Error('expected restored future')
    const done = await snapshot.resume([{ callId: snapshot.pendingCallIds[0], value: 42 }])
    t.true(done instanceof MontyComplete)
    t.is((done as MontyComplete).output, 42)
  } finally {
    await restored.close()
  }
})

/** Drives the same session through either public feed API. */
async function feed(
  session: MontySession,
  mode: 'run' | 'manual',
  code: string,
  options: FeedOptions = {},
): Promise<unknown> {
  if (mode === 'run') return session.feedRun(code, options)
  let snapshot = await session.feedStart(code, options)
  while (!(snapshot instanceof MontyComplete)) snapshot = await snapshot.resumeAuto()
  return snapshot.output
}

/** Lets a test settle a real host callback without a timer. */
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}
