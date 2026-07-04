import test from 'ava'

import { Monty, MontyRuntimeError, runMontyAsync } from '../ts/wasm.js'

// =============================================================================
// Basic async external function tests
// =============================================================================

test('runMontyAsync with sync external function', async (t) => {
  const m = new Monty('get_value()')

  const result = await runMontyAsync(m, {
    externalLookup: {
      get_value: () => 42,
    },
  })

  t.is(result, 42)
})

test('runMontyAsync with async external function', async (t) => {
  const m = new Monty('fetch_data()')

  const result = await runMontyAsync(m, {
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

test('runMontyAsync with multiple async calls', async (t) => {
  const m = new Monty(
    `
a = fetch_a()
b = fetch_b()
a + b
`,
    {},
  )

  const result = await runMontyAsync(m, {
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

test('runMontyAsync with inputs', async (t) => {
  const m = new Monty('multiply(x)', { inputs: ['x'] })

  const result = await runMontyAsync(m, {
    inputs: { x: 5 },
    externalLookup: {
      multiply: async (n: number) => n * 2,
    },
  })

  t.is(result, 10)
})

test('runMontyAsync with args and kwargs', async (t) => {
  const m = new Monty('process(1, 2, name="test")')

  const result = await runMontyAsync(m, {
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

test('runMontyAsync sync function throws exception', async (t) => {
  const m = new Monty('fail_sync()')

  class ValueError extends Error {
    override name = 'ValueError'
  }

  const error = await t.throwsAsync(
    runMontyAsync(m, {
      externalLookup: {
        fail_sync: () => {
          throw new ValueError('sync error')
        },
      },
    }),
  )

  t.true(error instanceof MontyRuntimeError)
})

test('runMontyAsync async function throws exception', async (t) => {
  const m = new Monty('fail_async()')

  class ValueError extends Error {
    override name = 'ValueError'
  }

  const error = await t.throwsAsync(
    runMontyAsync(m, {
      externalLookup: {
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
    {},
  )

  class ValueError extends Error {
    override name = 'ValueError'
  }

  const result = await runMontyAsync(m, {
    externalLookup: {
      might_fail: async () => {
        throw new ValueError('expected error')
      },
    },
  })

  t.is(result, 'caught')
})

test('runMontyAsync missing external function raises NameError', async (t) => {
  const m = new Monty('missing_func()')

  const error = await t.throwsAsync(runMontyAsync(m, { externalLookup: {} }))

  t.true(error instanceof MontyRuntimeError)
  t.true(error!.message.includes('NameError'))
})

test('runMontyAsync missing function caught in try/except', async (t) => {
  const m = new Monty(
    `
try:
    missing()
except NameError:
    result = 'caught'
result
`,
  )

  const result = await runMontyAsync(m, { externalLookup: {} })

  t.is(result, 'caught')
})

// =============================================================================
// Complex type tests
// =============================================================================

test('runMontyAsync returns complex types', async (t) => {
  const m = new Monty('get_data()')

  const result = await runMontyAsync(m, {
    externalLookup: {
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
  const m = new Monty('sum_list(items)', { inputs: ['items'] })

  const result = await runMontyAsync(m, {
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

test('runMontyAsync mixed sync and async functions', async (t) => {
  const m = new Monty(
    `
sync_result = sync_func()
async_result = async_func()
sync_result + async_result
`,
    {},
  )

  const result = await runMontyAsync(m, {
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

test('runMontyAsync chained async calls', async (t) => {
  const m = new Monty(
    `
first = get_first()
second = process(first)
finalize(second)
`,
    {},
  )

  const result = await runMontyAsync(m, {
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

// =============================================================================
// printCallback tests
// =============================================================================

test('runMontyAsync with printCallback', async (t) => {
  const m = new Monty('print("hello from async")')
  const output: string[] = []

  const result = await runMontyAsync(m, {
    printCallback: (stream, text) => {
      t.is(stream, 'stdout')
      output.push(text)
    },
  })

  t.is(result, null)
  t.deepEqual(output, ['hello from async', '\n'])
})

test('runMontyAsync printCallback with external functions', async (t) => {
  const m = new Monty('x = get_value()\nprint(f"got {x}")\nx')
  const output: string[] = []

  const result = await runMontyAsync(m, {
    externalLookup: {
      get_value: () => 42,
    },
    printCallback: (stream, text) => {
      t.is(stream, 'stdout')
      output.push(text)
    },
  })

  t.is(result, 42)
  t.deepEqual(output, ['got 42', '\n'])
})

test('runMontyAsync printCallback with multiple prints', async (t) => {
  const m = new Monty('print("a")\nprint("b")\nprint("c")')
  const output: string[] = []

  await runMontyAsync(m, {
    printCallback: (_stream, text) => {
      output.push(text)
    },
  })

  t.deepEqual(output, ['a', '\n', 'b', '\n', 'c', '\n'])
})

// =============================================================================
// externalLookup value resolution
// =============================================================================

test('runMontyAsync resolves a bare name to a value', async (t) => {
  const m = new Monty('x + 1')
  t.is(await runMontyAsync(m, { externalLookup: { x: 41 } }), 42)
})

test('runMontyAsync resolves a falsy value', async (t) => {
  // 0 is falsy but a present own key, so it must resolve rather than raise.
  const m = new Monty('n + 1')
  t.is(await runMontyAsync(m, { externalLookup: { n: 0 } }), 1)
})

test('runMontyAsync resolves null and undefined values to None', async (t) => {
  t.is(await runMontyAsync(new Monty('x is None'), { externalLookup: { x: null } }), true)
  t.is(await runMontyAsync(new Monty('y is None'), { externalLookup: { y: undefined } }), true)
})

test('runMontyAsync calling a proxy whose entry is now non-callable raises TypeError', async (t) => {
  // Calls dispatch by name against the *current* lookup on every call: the
  // first call replaces the entry with a plain value, so the second raises
  // what CPython would for calling that value.
  const lookup: Record<string, unknown> = {
    f: () => {
      lookup.f = 5
      return 1
    },
  }
  const error = await t.throwsAsync(runMontyAsync(new Monty('f()\nf()'), { externalLookup: lookup }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "TypeError: 'int' object is not callable")
})

test('runMontyAsync mixes an async function and a value', async (t) => {
  // The host awaits the JS promise and hands the sandbox the resolved value, so
  // the sandbox calls the function directly (no `await`). `url`/`suffix` are
  // non-callable externalLookup values resolved on bare-name reads.
  const m = new Monty('fetch_data(url) + suffix')
  const result = await runMontyAsync(m, {
    externalLookup: {
      fetch_data: async (u: string) => `<${u}>`,
      url: 'u',
      suffix: '!',
    },
  })
  t.is(result, '<u>!')
})

test('runMontyAsync inherited property name raises name error', async (t) => {
  // toString is inherited from Object.prototype, not an own key.
  const m = new Monty('toString')
  const error = await t.throwsAsync(runMontyAsync(m, { externalLookup: { present: 1 } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "NameError: name 'toString' is not defined")
})
