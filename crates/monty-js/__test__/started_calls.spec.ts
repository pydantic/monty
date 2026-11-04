// Calls to `async` host functions that the sandbox does not await at once:
// native and remote workers start them without suspending; the WebAssembly
// worker suspends at each, with the same results.

import { test } from 'vitest'
import { t } from './assertions.js'

import { ExternalModule, FunctionSnapshot, MontyRuntimeError } from '@pydantic/monty'
import { skipIfWasm } from './env.js'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

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

test('an async call runs on the host after the sandbox has moved on', async (ctx) => {
  skipIfWasm(ctx)
  const log: string[] = []
  const fetch = async (x: number) => {
    log.push(`fetch(${x})`)
    return x
  }
  const result = await run("a = fetch(1)\nprint('after the call')\nawait a", {
    externalLookup: { fetch },
    printCallback: (_stream, text) => log.push(text),
  })
  t.is(result, 1)
  // a suspended call would have run fetch before the print
  t.deepEqual(log, ['after the call\n', 'fetch(1)'])
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
  const step = await session.feedStart('a = fetch(1)\nawait a', { externalLookup: { fetch } })
  t.true(step instanceof FunctionSnapshot)
  t.is((step as FunctionSnapshot).functionName, 'fetch')
})
