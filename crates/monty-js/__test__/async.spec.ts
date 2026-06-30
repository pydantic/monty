// Async external functions: sync or async JS functions are passed the same
// way via `externalLookup`. A promise-returning function yields an
// awaitable in the sandbox (so the snippet uses `await fn()`), with the
// promise registered as a sandbox future and delivered automatically — plain
// `run(...)` covers everything the old in-process `runMontyAsync` helper did.

import test from 'ava'

import { MontyRuntimeError } from '../ts/index.js'
import { setupPool } from './helpers.js'

const { run } = setupPool(test)

// =============================================================================
// Basic async external function tests
// =============================================================================

test('run with sync external function', async (t) => {
  const result = await run('get_value()', {
    externalLookup: {
      get_value: () => 42,
    },
  })

  t.is(result, 42)
})

test('run with async external function', async (t) => {
  const result = await run('await fetch_data()', {
    externalLookup: {
      fetch_data: async () => {
        // Simulate async operation
        await new Promise((resolve) => setTimeout(resolve, 10))
        return 'async result'
      },
    },
  })

  t.is(result, 'async result')
})

test('run with multiple async calls', async (t) => {
  const code = `
a = await fetch_a()
b = await fetch_b()
a + b
`
  const result = await run(code, {
    externalLookup: {
      fetch_a: async () => {
        await new Promise((resolve) => setTimeout(resolve, 5))
        return 10
      },
      fetch_b: async () => {
        await new Promise((resolve) => setTimeout(resolve, 5))
        return 20
      },
    },
  })

  t.is(result, 30)
})

test('run async external function with inputs', async (t) => {
  const result = await run('await multiply(x)', {
    inputs: { x: 5 },
    externalLookup: {
      multiply: async (n: number) => n * 2,
    },
  })

  t.is(result, 10)
})

test('run async external function with args and kwargs', async (t) => {
  const result = await run('await process(1, 2, name="test")', {
    externalLookup: {
      process: async (a: number, b: number, kwargs: { name: string }) => {
        return `${kwargs.name}: ${a + b}`
      },
    },
  })

  t.is(result, 'test: 3')
})

// =============================================================================
// Error handling tests
// =============================================================================

test('sync external function throws exception', async (t) => {
  class ValueError extends Error {
    override name = 'ValueError'
  }

  const error = await t.throwsAsync(
    () =>
      run('fail_sync()', {
        externalLookup: {
          fail_sync: () => {
            throw new ValueError('sync error')
          },
        },
      }),
    { instanceOf: MontyRuntimeError },
  )

  t.is(error.message, 'ValueError: sync error')
})

test('async external function throws exception', async (t) => {
  class ValueError extends Error {
    override name = 'ValueError'
  }

  const error = await t.throwsAsync(
    () =>
      run('await fail_async()', {
        externalLookup: {
          fail_async: async () => {
            await new Promise((resolve) => setTimeout(resolve, 5))
            throw new ValueError('async error')
          },
        },
      }),
    { instanceOf: MontyRuntimeError },
  )

  t.is(error.message, 'ValueError: async error')
})

test('async external function exception caught in try/except', async (t) => {
  const code = `
try:
    await might_fail()
except ValueError:
    result = 'caught'
result
`
  class ValueError extends Error {
    override name = 'ValueError'
  }

  const result = await run(code, {
    externalLookup: {
      might_fail: async () => {
        throw new ValueError('expected error')
      },
    },
  })

  t.is(result, 'caught')
})

test('missing external function raises NameError', async (t) => {
  const error = await t.throwsAsync(() => run('missing_func()', { externalLookup: {} }), {
    instanceOf: MontyRuntimeError,
  })

  t.is(error.message, "NameError: name 'missing_func' is not defined")
})

test('missing external function caught in try/except', async (t) => {
  const code = `
try:
    missing()
except NameError:
    result = 'caught'
result
`
  t.is(await run(code, { externalLookup: {} }), 'caught')
})

// =============================================================================
// Complex type tests
// =============================================================================

test('async external function returns complex types', async (t) => {
  const result = (await run('await get_data()', {
    externalLookup: {
      get_data: async () => {
        return [1, 2, { key: 'value' }]
      },
    },
  })) as [number, number, Map<string, unknown>]

  t.true(Array.isArray(result))
  t.is(result[0], 1)
  t.is(result[1], 2)
  t.true(result[2] instanceof Map)
  t.is(result[2].get('key'), 'value')
})

test('async external function with list input', async (t) => {
  const result = await run('await sum_list(items)', {
    inputs: { items: [1, 2, 3, 4, 5] },
    externalLookup: {
      sum_list: async (items: number[]) => {
        return items.reduce((a, b) => a + b, 0)
      },
    },
  })

  t.is(result, 15)
})

// =============================================================================
// Mixed sync/async tests
// =============================================================================

test('mixed sync and async external functions', async (t) => {
  const code = `
sync_result = sync_func()
async_result = await async_func()
sync_result + async_result
`
  const result = await run(code, {
    externalLookup: {
      sync_func: () => 100,
      async_func: async () => {
        await new Promise((resolve) => setTimeout(resolve, 5))
        return 200
      },
    },
  })

  t.is(result, 300)
})

test('chained async external calls', async (t) => {
  const code = `
first = await get_first()
second = await process(first)
await finalize(second)
`
  const result = await run(code, {
    externalLookup: {
      get_first: async () => 'hello',
      process: async (s: string) => s.toUpperCase(),
      finalize: async (s: string) => `${s}!`,
    },
  })

  t.is(result, 'HELLO!')
})

// =============================================================================
// No external functions tests
// =============================================================================

test('run without external functions', async (t) => {
  t.is(await run('1 + 2', {}), 3)
})

test('run pure computation', async (t) => {
  const code = `
def factorial(n):
    if n <= 1:
        return 1
    return n * factorial(n - 1)
factorial(5)
`
  t.is(await run(code), 120)
})

// =============================================================================
// printCallback tests
// =============================================================================

test('run with printCallback', async (t) => {
  const output: string[] = []

  const result = await run('print("hello from async")', {
    printCallback: (stream, text) => {
      t.is(stream, 'stdout')
      output.push(text)
    },
  })

  t.is(result, null)
  // Output is line-buffered: assert the joined text, not the chunking
  t.is(output.join(''), 'hello from async\n')
})

test('printCallback with external functions', async (t) => {
  const output: string[] = []

  const result = await run('x = get_value()\nprint(f"got {x}")\nx', {
    externalLookup: {
      get_value: () => 42,
    },
    printCallback: (stream, text) => {
      t.is(stream, 'stdout')
      output.push(text)
    },
  })

  t.is(result, 42)
  t.is(output.join(''), 'got 42\n')
})

test('printCallback with multiple prints', async (t) => {
  const output: string[] = []

  await run('print("a")\nprint("b")\nprint("c")', {
    printCallback: (_stream, text) => {
      output.push(text)
    },
  })

  t.is(output.join(''), 'a\nb\nc\n')
})
