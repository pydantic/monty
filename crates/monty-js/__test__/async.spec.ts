import test from 'ava'

import { Monty, MontyRuntimeError, runMontyAsync } from '../wrapper'

// =============================================================================
// Basic async external function tests
// =============================================================================

test('runMontyAsync with sync external function', async (t) => {
  const m = new Monty('get_value()', { externalFunctions: ['get_value'] })

  const result = await runMontyAsync(m, {
    externalFunctions: {
      get_value: () => 42,
    },
  })

  t.is(result, 42)
})

test('runMontyAsync with async external function', async (t) => {
  const m = new Monty('fetch_data()', { externalFunctions: ['fetch_data'] })

  const result = await runMontyAsync(m, {
    externalFunctions: {
      fetch_data: async () => {
        // Simulate async operation
        await new Promise((resolve) => setTimeout(resolve, 10))
        return 'async result'
      },
    },
  })

  t.is(result, 'async result')
})

test('runMontyAsync with multiple async calls', async (t) => {
  const m = new Monty(
    `
a = fetch_a()
b = fetch_b()
a + b
`,
    { externalFunctions: ['fetch_a', 'fetch_b'] },
  )

  const result = await runMontyAsync(m, {
    externalFunctions: {
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

test('runMontyAsync with inputs', async (t) => {
  const m = new Monty('multiply(x)', { inputs: ['x'], externalFunctions: ['multiply'] })

  const result = await runMontyAsync(m, {
    inputs: { x: 5 },
    externalFunctions: {
      multiply: async (n: number) => n * 2,
    },
  })

  t.is(result, 10)
})

test('runMontyAsync with args and kwargs', async (t) => {
  const m = new Monty('process(1, 2, name="test")', { externalFunctions: ['process'] })

  const result = await runMontyAsync(m, {
    externalFunctions: {
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

test('runMontyAsync sync function throws exception', async (t) => {
  const m = new Monty('fail_sync()', { externalFunctions: ['fail_sync'] })

  class ValueError extends Error {
    override name = 'ValueError'
  }

  const error = await t.throwsAsync(
    runMontyAsync(m, {
      externalFunctions: {
        fail_sync: () => {
          throw new ValueError('sync error')
        },
      },
    }),
  )

  t.true(error instanceof MontyRuntimeError)
})

test('runMontyAsync async function throws exception', async (t) => {
  const m = new Monty('fail_async()', { externalFunctions: ['fail_async'] })

  class ValueError extends Error {
    override name = 'ValueError'
  }

  const error = await t.throwsAsync(
    runMontyAsync(m, {
      externalFunctions: {
        fail_async: async () => {
          await new Promise((resolve) => setTimeout(resolve, 5))
          throw new ValueError('async error')
        },
      },
    }),
  )

  t.true(error instanceof MontyRuntimeError)
})

test('runMontyAsync exception caught in try/except', async (t) => {
  const m = new Monty(
    `
try:
    might_fail()
except ValueError:
    result = 'caught'
result
`,
    { externalFunctions: ['might_fail'] },
  )

  class ValueError extends Error {
    override name = 'ValueError'
  }

  const result = await runMontyAsync(m, {
    externalFunctions: {
      might_fail: async () => {
        throw new ValueError('expected error')
      },
    },
  })

  t.is(result, 'caught')
})

test('runMontyAsync missing external function', async (t) => {
  const m = new Monty('missing_func()', { externalFunctions: ['missing_func'] })

  const error = await t.throwsAsync(runMontyAsync(m, { externalFunctions: {} }))

  t.true(error instanceof MontyRuntimeError)
})

test('runMontyAsync missing function caught in try/except', async (t) => {
  const m = new Monty(
    `
try:
    missing()
except KeyError:
    result = 'key error caught'
result
`,
    { externalFunctions: ['missing'] },
  )

  const result = await runMontyAsync(m, { externalFunctions: {} })

  t.is(result, 'key error caught')
})

// =============================================================================
// Complex type tests
// =============================================================================

test('runMontyAsync returns complex types', async (t) => {
  const m = new Monty('get_data()', { externalFunctions: ['get_data'] })

  const result = await runMontyAsync(m, {
    externalFunctions: {
      get_data: async () => {
        return [1, 2, { key: 'value' }]
      },
    },
  })

  t.true(Array.isArray(result))
  t.is(result[0], 1)
  t.is(result[1], 2)
  t.true(result[2] instanceof Map)
  t.is(result[2].get('key'), 'value')
})

test('runMontyAsync with list input', async (t) => {
  const m = new Monty('sum_list(items)', { inputs: ['items'], externalFunctions: ['sum_list'] })

  const result = await runMontyAsync(m, {
    inputs: { items: [1, 2, 3, 4, 5] },
    externalFunctions: {
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

test('runMontyAsync mixed sync and async functions', async (t) => {
  const m = new Monty(
    `
sync_result = sync_func()
async_result = async_func()
sync_result + async_result
`,
    { externalFunctions: ['sync_func', 'async_func'] },
  )

  const result = await runMontyAsync(m, {
    externalFunctions: {
      sync_func: () => 100,
      async_func: async () => {
        await new Promise((resolve) => setTimeout(resolve, 5))
        return 200
      },
    },
  })

  t.is(result, 300)
})

test('runMontyAsync chained async calls', async (t) => {
  const m = new Monty(
    `
first = get_first()
second = process(first)
finalize(second)
`,
    { externalFunctions: ['get_first', 'process', 'finalize'] },
  )

  const result = await runMontyAsync(m, {
    externalFunctions: {
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

test('runMontyAsync without external functions', async (t) => {
  const m = new Monty('1 + 2')

  const result = await runMontyAsync(m, {})

  t.is(result, 3)
})

test('runMontyAsync pure computation', async (t) => {
  const m = new Monty(
    `
def factorial(n):
    if n <= 1:
        return 1
    return n * factorial(n - 1)
factorial(5)
`,
  )

  const result = await runMontyAsync(m)

  t.is(result, 120)
})
