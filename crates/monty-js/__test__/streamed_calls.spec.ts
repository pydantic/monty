// Calls to `async` host functions that the sandbox does not await at once:
// with `streamAsyncCalls`, native and remote workers start them without
// suspending; the WebAssembly worker suspends at each, with the same results.

import { test } from 'vitest'
import { t } from './assertions.js'

import { ExternalModule, FunctionSnapshot, Monty, MontyRuntimeError } from '@pydantic/monty'
import { skipIfWasm } from './env.js'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool({ streamAsyncCalls: true })

class ValueError extends Error {
  override name = 'ValueError'
}

// === Results match on every backend ===

test('async host functions run concurrently under gather', async () => {
  let release: () => void = () => {}
  const ready = new Promise<void>((resolve) => (release = resolve))
  const externalLookup = {
    first: async () => {
      await ready
      return 1
    },
    second: async () => {
      release()
      return 2
    },
  }
  t.deepEqual(await run('import asyncio\nawait asyncio.gather(first(), second())', { externalLookup }), [1, 2])
  const wrapped =
    'import asyncio\nasync def a():\n    return await first()\nasync def b():\n    return await second()\n' +
    'await asyncio.gather(a(), b())'
  t.deepEqual(await run(wrapped, { externalLookup }), [1, 2])
})

test('an error from an async host function is raised at the await', async () => {
  const fail = async (): Promise<never> => {
    throw new ValueError('intentional error')
  }
  const code = "a = fail()\nr = 'called'\ntry:\n    await a\nexcept ValueError as e:\n    r += ': ' + str(e)\nr"
  t.is(await run(code, { externalLookup: { fail } }), 'called: intentional error')
})

test('async functions on an ExternalModule', async () => {
  const fetch = async (x: number) => x + 1
  await using session = await pool().checkout({ externalModules: { tools: new ExternalModule({ fetch }) } })
  const code = 'import asyncio\nimport tools\nawait asyncio.gather(tools.fetch(1), tools.fetch(2))'
  t.deepEqual(await session.feedRun(code), [2, 3])
})

test('each started call counts against maxSuspensions', async () => {
  const fetch = async (x: number) => x
  const error = await t.throwsAsync(
    () =>
      run('import asyncio\nawait asyncio.gather(*[fetch(i) for i in range(3)])', {
        limits: { maxSuspensions: 2 },
        externalLookup: { fetch },
      }),
    { instanceOf: MontyRuntimeError },
  )
  t.is(error.message, 'RuntimeError: suspension limit 2 exceeded')
})

test('a call awaited at once costs one suspension', async () => {
  const fetch = async (x: number) => x
  const result = await run('a = await fetch(1)\nb = await fetch(2)\na + b', {
    limits: { maxSuspensions: 2 },
    externalLookup: { fetch },
  })
  t.is(result, 3)
})

// === Native and remote workers start calls without suspending ===

// Times the call from inside the sandbox, then works long enough for a call
// started mid-turn to run before the turn ends.
const TIMED_CALL =
  'import time\nt0 = time.monotonic()\na = fetch(1)\nelapsed = time.monotonic() - t0\n' +
  "for _ in range(2_000_000):\n    pass\nprint('after the loop')\n[await a, elapsed < 0.25]"

/** An async host function that blocks the host for 300ms before its first await. */
function blockingFetch(log: string[]): (x: number) => Promise<number> {
  return async (x: number) => {
    log.push(`fetch(${x})`)
    const until = Date.now() + 300
    while (Date.now() < until) {
      // busy-wait: the sandbox only waits for this if the call suspended it
    }
    return x
  }
}

test('a streamed call starts on the host while the sandbox keeps running', async (ctx) => {
  skipIfWasm(ctx)
  const log: string[] = []
  const result = await run(TIMED_CALL, {
    externalLookup: { fetch: blockingFetch(log) },
    printCallback: (_stream, text) => log.push(text),
  })
  // the call returned at once, and the host ran it before the sandbox's loop ended
  t.deepEqual(result, [1, true])
  t.deepEqual(log, ['fetch(1)', 'after the loop\n'])
})

test('a failed print stops started calls from running on the host', async (ctx) => {
  skipIfWasm(ctx)
  let calls = 0
  const effect = async () => {
    calls += 1
  }
  const error = await t.throwsAsync(() =>
    run("print('x')\na = effect()\nawait a", {
      externalLookup: { effect },
      printCallback: () => {
        throw new Error('print rejected')
      },
    }),
  )
  t.is(error.message, 'print rejected')
  t.is(calls, 0)
})

test('a feedRun refused while another is suspended leaves that feed its calls', async (ctx) => {
  skipIfWasm(ctx)
  await using session = await pool().checkout()
  let release: () => void = () => {}
  const gate = new Promise<void>((resolve) => (release = resolve))
  const fetch = async (x: number) => {
    await gate
    return x
  }
  // A suspends awaiting its first call, then streams a second call once resumed
  const first = session.feedRun('a = fetch(1)\nx = await a\nb = fetch(2)\nx + await b', { externalLookup: { fetch } })
  await new Promise((resolve) => setTimeout(resolve, 100))
  // observed at once: it is refused while `first` is still running
  const second = t.throwsAsync(() => session.feedRun('1', { externalLookup: { fetch } }))
  release()
  t.is(await first, 3)
  const error = await second
  t.is(error.message, 'monty worker protocol error: feed called while a suspension is awaiting an answer')
})

test('a subprocess pool leaves streamAsyncCalls off, so async calls suspend', async (ctx) => {
  skipIfWasm(ctx)
  const log: string[] = []
  await using defaults = await Monty.create()
  await using session = await defaults.checkout()
  const result = await session.feedRun(TIMED_CALL, {
    externalLookup: { fetch: blockingFetch(log) },
    printCallback: (_stream, text) => log.push(text),
  })
  // the sandbox waited at the call for the host's 300ms
  t.deepEqual(result, [1, false])
  t.deepEqual(log, ['fetch(1)', 'after the loop\n'])
})

test('a sync host function still suspends at the call', async (ctx) => {
  skipIfWasm(ctx)
  const log: string[] = []
  const fetch = (x: number) => {
    log.push(`fetch(${x})`)
    return x
  }
  const result = await run("a = fetch(1)\nprint('after the call')\na", {
    externalLookup: { fetch },
    printCallback: (_stream, text) => log.push(text),
  })
  t.is(result, 1)
  t.deepEqual(log, ['fetch(1)', 'after the call\n'])
})

test('an async function the host cannot find raises NameError at the await', async (ctx) => {
  skipIfWasm(ctx)
  const code = 'a = fetch()\ntry:\n    await a\nexcept NameError as e:\n    r = str(e)\nr'
  const result = await run(code, { inputs: { fetch: async function fetch() {} } })
  t.is(result, "name 'fetch' is not defined")
})

test('feedStart suspends at every call to an async function', async () => {
  const fetch = async (x: number) => x * 2
  await using session = await pool().checkout()
  const code = 'a = fetch(1)\nb = fetch(2)\nawait a + await b'
  const first = await session.feedStart(code, { externalLookup: { fetch } })
  t.true(first instanceof FunctionSnapshot)
  t.deepEqual((first as FunctionSnapshot).args, [1])
  const second = await (first as FunctionSnapshot).resume(2)
  t.true(second instanceof FunctionSnapshot)
  t.deepEqual((second as FunctionSnapshot).args, [2])
})
