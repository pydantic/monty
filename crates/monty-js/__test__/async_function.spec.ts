// `asyncFunction()`: marked host functions return an awaitable at once and raise
// at the `await`. Native and remote workers start the calls without suspending;
// the WebAssembly worker suspends at each, with the same results.

import { test } from 'vitest'
import { t } from './assertions.js'

import { asyncFunction, ExternalModule, MontyRuntimeError } from '@pydantic/monty'
import { skipIfWasm } from './env.js'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

class ValueError extends Error {
  override name = 'ValueError'
}

// === Results match on every backend ===

test('marked coroutines run concurrently under gather', async () => {
  let release: () => void = () => {}
  const ready = new Promise<void>((resolve) => (release = resolve))
  const externalLookup = {
    first: asyncFunction(async () => {
      await ready
      return 1
    }),
    second: asyncFunction(async () => {
      release()
      return 2
    }),
  }
  t.deepEqual(await run('import asyncio\nawait asyncio.gather(first(), second())', { externalLookup }), [1, 2])
  const wrapped =
    'import asyncio\nasync def a():\n    return await first()\nasync def b():\n    return await second()\n' +
    'await asyncio.gather(a(), b())'
  t.deepEqual(await run(wrapped, { externalLookup }), [1, 2])
})

test('a marked function returning a plain value still returns an awaitable', async () => {
  const double = asyncFunction((x: number) => x * 2)
  t.deepEqual(await run('a = double(1)\nb = double(2)\n[await a, await b]', { externalLookup: { double } }), [2, 4])
})

test('an error from a marked function is raised at the await', async () => {
  const fail = asyncFunction((): never => {
    throw new ValueError('intentional error')
  })
  const code = "a = fail()\nr = 'called'\ntry:\n    await a\nexcept ValueError as e:\n    r += ': ' + str(e)\nr"
  t.is(await run(code, { externalLookup: { fail } }), 'called: intentional error')
})

test('marked functions on an ExternalModule', async () => {
  const fetch = asyncFunction(async (x: number) => x + 1)
  await using session = await pool().checkout({ externalModules: { tools: new ExternalModule({ fetch }) } })
  const code = 'import asyncio\nimport tools\nawait asyncio.gather(tools.fetch(1), tools.fetch(2))'
  t.deepEqual(await session.feedRun(code), [2, 3])
})

test('each marked call counts against maxSuspensions', async () => {
  const fetch = asyncFunction(async (x: number) => x)
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

test('asyncFunction keeps the name and behaviour of the function it wraps', () => {
  function fetch(x: number): number {
    return x
  }
  const marked = asyncFunction(fetch)
  t.is(marked.name, 'fetch')
  t.is(marked(3), 3)
  t.not(marked, fetch)
})

test('asyncFunction requires a function', () => {
  const error = t.throws(() => asyncFunction(1 as never), { instanceOf: TypeError })
  t.is(error.message, 'asyncFunction() argument must be a function, not number')
})

// === Native and remote workers start calls without suspending ===

test('a marked call runs on the host after the sandbox has moved on', async (ctx) => {
  skipIfWasm(ctx)
  const log: string[] = []
  const fetch = asyncFunction(async (x: number) => {
    log.push(`fetch(${x})`)
    return x
  })
  const result = await run("a = fetch(1)\nprint('after the call')\nawait a", {
    externalLookup: { fetch },
    printCallback: (_stream, text) => log.push(text),
  })
  t.is(result, 1)
  // a suspended call would have run fetch before the print
  t.deepEqual(log, ['after the call\n', 'fetch(1)'])
})

test('a marked function the host cannot find raises NameError at the await', async (ctx) => {
  skipIfWasm(ctx)
  const code = 'a = fetch()\ntry:\n    await a\nexcept NameError as e:\n    r = str(e)\nr'
  const result = await run(code, { inputs: { fetch: asyncFunction(function fetch() {}) } })
  t.is(result, "name 'fetch' is not defined")
})

test('feedStart rejects a marked call it cannot hand over as a snapshot', async (ctx) => {
  skipIfWasm(ctx)
  const double = asyncFunction((x: number) => x * 2)
  await using session = await pool().checkout()
  const error = await t.throwsAsync(() => session.feedStart('await double(1)', { externalLookup: { double } }))
  t.is(error.message, 'functions marked with asyncFunction() are not supported by feedStart(); use feedRun()')
})
