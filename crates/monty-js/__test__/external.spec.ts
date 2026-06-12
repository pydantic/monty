import test from 'ava'

import { MontyRuntimeError } from '../ts/index.js'
import { setupPool } from './helpers.js'

const { run } = setupPool(test)

// =============================================================================
// Basic external function tests
// =============================================================================

test('external function no args', async (t) => {
  const noop = (...args: unknown[]) => {
    t.deepEqual(args, [])
    return 'called'
  }

  t.is(await run('noop()', { externalFunctions: { noop } }), 'called')
})

test('external function positional args', async (t) => {
  const func = (...args: unknown[]) => {
    t.deepEqual(args, [1, 2, 3])
    return 'ok'
  }

  t.is(await run('func(1, 2, 3)', { externalFunctions: { func } }), 'ok')
})

test('external function kwargs only', async (t) => {
  const func = (...args: unknown[]) => {
    // kwargs are passed as the last argument as an object
    t.deepEqual(args, [{ a: 1, b: 'two' }])
    return 'ok'
  }

  t.is(await run('func(a=1, b="two")', { externalFunctions: { func } }), 'ok')
})

test('external function mixed args kwargs', async (t) => {
  const func = (...args: unknown[]) => {
    // positional args followed by kwargs object
    t.deepEqual(args, [1, 2, { x: 'hello', y: true }])
    return 'ok'
  }

  t.is(await run('func(1, 2, x="hello", y=True)', { externalFunctions: { func } }), 'ok')
})

test('external function complex types', async (t) => {
  const func = (...args: unknown[]) => {
    t.deepEqual(args[0], [1, 2])
    // Dicts are returned as Maps
    t.true(args[1] instanceof Map)
    t.is((args[1] as Map<string, string>).get('key'), 'value')
    return 'ok'
  }

  t.is(await run('func([1, 2], {"key": "value"})', { externalFunctions: { func } }), 'ok')
})

test('external function returns none', async (t) => {
  const do_nothing = () => {
    // returns undefined which becomes None
  }

  t.is(await run('do_nothing()', { externalFunctions: { do_nothing } }), null)
})

test('external function returns complex type', async (t) => {
  const get_data = () => {
    return { a: [1, 2, 3], b: { nested: true } }
  }

  const result = (await run('get_data()', { externalFunctions: { get_data } })) as Map<string, unknown>
  // Plain objects become Maps
  t.true(result instanceof Map)
  t.deepEqual(result.get('a'), [1, 2, 3])
  const nested = result.get('b') as Map<string, unknown>
  t.true(nested instanceof Map)
  t.is(nested.get('nested'), true)
})

// =============================================================================
// Multiple external functions tests
// =============================================================================

test('multiple external functions', async (t) => {
  const add = (a: number, b: number) => {
    t.is(a, 1)
    t.is(b, 2)
    return a + b
  }

  const mul = (a: number, b: number) => {
    t.is(a, 3)
    t.is(b, 4)
    return a * b
  }

  const result = await run('add(1, 2) + mul(3, 4)', { externalFunctions: { add, mul } })
  t.is(result, 15) // 3 + 12
})

test('external function called multiple times', async (t) => {
  let callCount = 0

  const counter = () => {
    callCount += 1
    return callCount
  }

  const result = await run('counter() + counter() + counter()', { externalFunctions: { counter } })
  t.is(result, 6) // 1 + 2 + 3
  t.is(callCount, 3)
})

test('external function with input', async (t) => {
  const process = (x: number) => {
    t.is(x, 5)
    return x * 10
  }

  t.is(await run('process(x)', { inputs: { x: 5 }, externalFunctions: { process } }), 50)
})

// =============================================================================
// Error handling tests
// =============================================================================

test('undeclared external function raises name error', async (t) => {
  const error = await t.throwsAsync(() => run('missing()'), { instanceOf: MontyRuntimeError })
  t.is(error.message, "NameError: name 'missing' is not defined")
})

test('undeclared function raises name error', async (t) => {
  const error = await t.throwsAsync(() => run('unknown_func()'), { instanceOf: MontyRuntimeError })
  t.is(error.message, "NameError: name 'unknown_func' is not defined")
})

test('external function raises exception', async (t) => {
  const fail = () => {
    const error = new Error('intentional error')
    error.name = 'ValueError'
    throw error
  }

  const error = await t.throwsAsync(() => run('fail()', { externalFunctions: { fail } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'ValueError: intentional error')
})

test('external function wrong name raises name error', async (t) => {
  // When 'foo' is called but only 'bar' is provided, foo is a NameError
  const bar = () => 1

  const error = await t.throwsAsync(() => run('foo()', { externalFunctions: { bar } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "NameError: name 'foo' is not defined")
})

test('external function exception caught by try except', async (t) => {
  const code = `
try:
    fail()
except ValueError:
    caught = True
caught
`
  const fail = () => {
    const error = new Error('caught error')
    error.name = 'ValueError'
    throw error
  }

  t.is(await run(code, { externalFunctions: { fail } }), true)
})

test('external function exception type preserved', async (t) => {
  const fail = () => {
    const error = new Error('type error message')
    error.name = 'TypeError'
    throw error
  }

  const error = await t.throwsAsync(() => run('fail()', { externalFunctions: { fail } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'TypeError: type error message')
})

// =============================================================================
// Exception hierarchy tests
// =============================================================================

// A thrown JS error's `name` passes through as the Python exception type when
// it matches one of monty's exception types (the full ExcType list); anything
// else becomes a plain RuntimeError. The second column is the type the
// sandbox actually raises.
const exceptionTypes: Array<[string, string]> = [
  ['ZeroDivisionError', 'ZeroDivisionError'],
  ['OverflowError', 'OverflowError'],
  ['ArithmeticError', 'ArithmeticError'],
  ['NotImplementedError', 'NotImplementedError'],
  ['RecursionError', 'RecursionError'],
  ['RuntimeError', 'RuntimeError'],
  ['KeyError', 'KeyError'],
  ['IndexError', 'IndexError'],
  ['LookupError', 'LookupError'],
  ['ValueError', 'ValueError'],
  ['TypeError', 'TypeError'],
  ['AttributeError', 'AttributeError'],
  ['NameError', 'NameError'],
  ['AssertionError', 'AssertionError'],
  ['SomeCustomError', 'RuntimeError'],
]

for (const [jsName, pythonType] of exceptionTypes) {
  test(`external function exception hierarchy - ${jsName}`, async (t) => {
    const fail = () => {
      const error = new Error('test message')
      error.name = jsName
      throw error
    }

    const error = await t.throwsAsync(() => run('fail()', { externalFunctions: { fail } }), {
      instanceOf: MontyRuntimeError,
    })
    t.is(error.exception.typeName, pythonType)
    t.is(error.exception.message, 'test message')
  })
}

// =============================================================================
// Exception caught by parent tests
// =============================================================================

const parentChildPairs: Array<[string, string]> = [
  ['ZeroDivisionError', 'ArithmeticError'],
  ['OverflowError', 'ArithmeticError'],
  ['NotImplementedError', 'RuntimeError'],
  ['RecursionError', 'RuntimeError'],
  ['KeyError', 'LookupError'],
  ['IndexError', 'LookupError'],
]

for (const [childType, parentType] of parentChildPairs) {
  test(`external function exception caught by parent - ${childType} caught by ${parentType}`, async (t) => {
    const code = `
try:
    fail()
except ${parentType}:
    caught = 'parent'
except ${childType}:
    caught = 'child'
caught
`
    const fail = () => {
      const error = new Error('test')
      error.name = childType
      throw error
    }

    // Child exception should be caught by parent handler (which comes first)
    t.is(await run(code, { externalFunctions: { fail } }), 'parent')
  })
}

// =============================================================================
// Exception in various contexts
// =============================================================================

test('external function exception in expression', async (t) => {
  const fail = () => {
    const error = new Error('mid-expression error')
    error.name = 'RuntimeError'
    throw error
  }

  const error = await t.throwsAsync(() => run('1 + fail() + 2', { externalFunctions: { fail } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'RuntimeError: mid-expression error')
})

test('external function exception after successful call', async (t) => {
  const code = `
a = success()
b = fail()
a + b
`
  const success = () => 10

  const fail = () => {
    const error = new Error('second call fails')
    error.name = 'ValueError'
    throw error
  }

  const error = await t.throwsAsync(() => run(code, { externalFunctions: { success, fail } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'ValueError: second call fails')
})

test('external function exception with finally', async (t) => {
  const code = `
finally_ran = False
try:
    fail()
except ValueError:
    pass
finally:
    finally_ran = True
finally_ran
`
  const fail = () => {
    const error = new Error('error')
    error.name = 'ValueError'
    throw error
  }

  t.is(await run(code, { externalFunctions: { fail } }), true)
})

// =============================================================================
// Unconvertible return values
// =============================================================================

// A return value the wire cannot represent must surface as a catchable
// in-sandbox error — never desynchronize the protocol or wedge the session.
test('external function returning a malformed marker object', async (t) => {
  const code = `
try:
    bad()
except TypeError as exc:
    caught = str(exc)
caught
`
  // a Dataclass marker without its fieldNames array
  const bad = () => ({ __monty_type__: 'Dataclass', name: 'Broken' })
  t.is(
    await run(code, { externalFunctions: { bad } }),
    "Object property 'typeId' type mismatch. Expect value to be BigInt, but received Undefined",
  )
})

test('external function returning a symbol', async (t) => {
  const error = await t.throwsAsync(() => run('bad()', { externalFunctions: { bad: () => Symbol('nope') } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'TypeError: Cannot convert JS Symbol to Monty value')
})
