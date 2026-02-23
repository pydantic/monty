import test from 'ava'

import { Monty } from '../wrapper'
import { isRuntimeError } from './exceptions.spec'

// =============================================================================
// Basic external function tests
// =============================================================================

test('external function no args', (t) => {
  const m = new Monty('noop()', { externalFunctions: ['noop'] })

  const noop = (...args: unknown[]) => {
    t.deepEqual(args, [])
    return 'called'
  }

  const result = m.run({ externalFunctions: { noop } })
  t.is(result, 'called')
})

test('external function positional args', (t) => {
  const m = new Monty('func(1, 2, 3)', { externalFunctions: ['func'] })

  const func = (...args: unknown[]) => {
    t.deepEqual(args, [1, 2, 3])
    return 'ok'
  }

  t.is(m.run({ externalFunctions: { func } }), 'ok')
})

test('external function kwargs only', (t) => {
  const m = new Monty('func(a=1, b="two")', { externalFunctions: ['func'] })

  const func = (...args: unknown[]) => {
    // kwargs are passed as the last argument as an object
    t.deepEqual(args, [{ a: 1, b: 'two' }])
    return 'ok'
  }

  t.is(m.run({ externalFunctions: { func } }), 'ok')
})

test('external function mixed args kwargs', (t) => {
  const m = new Monty('func(1, 2, x="hello", y=True)', { externalFunctions: ['func'] })

  const func = (...args: unknown[]) => {
    // positional args followed by kwargs object
    t.deepEqual(args, [1, 2, { x: 'hello', y: true }])
    return 'ok'
  }

  t.is(m.run({ externalFunctions: { func } }), 'ok')
})

test('external function complex types', (t) => {
  const m = new Monty('func([1, 2], {"key": "value"})', { externalFunctions: ['func'] })

  const func = (...args: unknown[]) => {
    t.deepEqual(args[0], [1, 2])
    // Dicts are returned as Maps
    t.true(args[1] instanceof Map)
    t.is((args[1] as Map<string, string>).get('key'), 'value')
    return 'ok'
  }

  t.is(m.run({ externalFunctions: { func } }), 'ok')
})

test('external function returns none', (t) => {
  const m = new Monty('do_nothing()', { externalFunctions: ['do_nothing'] })

  const do_nothing = () => {
    // returns undefined which becomes None
  }

  t.is(m.run({ externalFunctions: { do_nothing } }), null)
})

test('external function returns complex type', (t) => {
  const m = new Monty('get_data()', { externalFunctions: ['get_data'] })

  const get_data = () => {
    return { a: [1, 2, 3], b: { nested: true } }
  }

  const result = m.run({ externalFunctions: { get_data } })
  // Plain objects become Maps
  t.true(result instanceof Map)
  t.deepEqual(result.get('a'), [1, 2, 3])
  const nested = result.get('b')
  t.true(nested instanceof Map)
  t.is(nested.get('nested'), true)
})

// =============================================================================
// Multiple external functions tests
// =============================================================================

test('multiple external functions', (t) => {
  const m = new Monty('add(1, 2) + mul(3, 4)', { externalFunctions: ['add', 'mul'] })

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

  const result = m.run({ externalFunctions: { add, mul } })
  t.is(result, 15) // 3 + 12
})

test('external function called multiple times', (t) => {
  const m = new Monty('counter() + counter() + counter()', { externalFunctions: ['counter'] })

  let callCount = 0

  const counter = () => {
    callCount += 1
    return callCount
  }

  const result = m.run({ externalFunctions: { counter } })
  t.is(result, 6) // 1 + 2 + 3
  t.is(callCount, 3)
})

test('external function with input', (t) => {
  const m = new Monty('process(x)', { inputs: ['x'], externalFunctions: ['process'] })

  const process = (x: number) => {
    t.is(x, 5)
    return x * 10
  }

  t.is(m.run({ inputs: { x: 5 }, externalFunctions: { process } }), 50)
})

// =============================================================================
// Error handling tests
// =============================================================================

test('external function not provided raises', (t) => {
  const m = new Monty('missing()', { externalFunctions: ['missing'] })

  const error = t.throws(() => m.run(), { message: /no externalFunctions provided/i })
  t.truthy(error)
})

test('undeclared function raises name error', (t) => {
  const m = new Monty('unknown_func()')

  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, "NameError: name 'unknown_func' is not defined")
})

test('external function raises exception', (t) => {
  const m = new Monty('fail()', { externalFunctions: ['fail'] })

  const fail = () => {
    const error = new Error('intentional error')
    error.name = 'ValueError'
    throw error
  }

  const error = t.throws(() => m.run({ externalFunctions: { fail } }), isRuntimeError)
  t.true(error.message.includes('ValueError'))
  t.true(error.message.includes('intentional error'))
})

test('external function wrong name raises', (t) => {
  const m = new Monty('foo()', { externalFunctions: ['foo'] })

  const bar = () => 1

  const error = t.throws(() => m.run({ externalFunctions: { bar } }), isRuntimeError)
  t.true(error.message.includes('KeyError'))
  t.true(error.message.includes('foo'))
})

test('external function exception caught by try except', (t) => {
  const code = `
try:
    fail()
except ValueError:
    caught = True
caught
`
  const m = new Monty(code, { externalFunctions: ['fail'] })

  const fail = () => {
    const error = new Error('caught error')
    error.name = 'ValueError'
    throw error
  }

  t.is(m.run({ externalFunctions: { fail } }), true)
})

test('external function exception type preserved', (t) => {
  const m = new Monty('fail()', { externalFunctions: ['fail'] })

  const fail = () => {
    const error = new Error('type error message')
    error.name = 'TypeError'
    throw error
  }

  const error = t.throws(() => m.run({ externalFunctions: { fail } }), isRuntimeError)
  t.true(error.message.includes('TypeError'))
  t.true(error.message.includes('type error message'))
})

// =============================================================================
// Exception hierarchy tests
// =============================================================================

const exceptionTypes = [
  'ZeroDivisionError',
  'OverflowError',
  'ArithmeticError',
  'NotImplementedError',
  'RecursionError',
  'RuntimeError',
  'KeyError',
  'IndexError',
  'LookupError',
  'ValueError',
  'TypeError',
  'AttributeError',
  'NameError',
  'AssertionError',
]

for (const exceptionType of exceptionTypes) {
  test(`external function exception hierarchy - ${exceptionType}`, (t) => {
    const m = new Monty('fail()', { externalFunctions: ['fail'] })

    const fail = () => {
      const error = new Error('test message')
      error.name = exceptionType
      throw error
    }

    const error = t.throws(() => m.run({ externalFunctions: { fail } }), isRuntimeError)
    t.true(error.message.includes(exceptionType))
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
  test(`external function exception caught by parent - ${childType} caught by ${parentType}`, (t) => {
    const code = `
try:
    fail()
except ${parentType}:
    caught = 'parent'
except ${childType}:
    caught = 'child'
caught
`
    const m = new Monty(code, { externalFunctions: ['fail'] })

    const fail = () => {
      const error = new Error('test')
      error.name = childType
      throw error
    }

    // Child exception should be caught by parent handler (which comes first)
    t.is(m.run({ externalFunctions: { fail } }), 'parent')
  })
}

// =============================================================================
// Exception in various contexts
// =============================================================================

test('external function exception in expression', (t) => {
  const m = new Monty('1 + fail() + 2', { externalFunctions: ['fail'] })

  const fail = () => {
    const error = new Error('mid-expression error')
    error.name = 'RuntimeError'
    throw error
  }

  const error = t.throws(() => m.run({ externalFunctions: { fail } }), isRuntimeError)
  t.true(error.message.includes('RuntimeError'))
  t.true(error.message.includes('mid-expression error'))
})

test('external function exception after successful call', (t) => {
  const code = `
a = success()
b = fail()
a + b
`
  const m = new Monty(code, { externalFunctions: ['success', 'fail'] })

  const success = () => 10

  const fail = () => {
    const error = new Error('second call fails')
    error.name = 'ValueError'
    throw error
  }

  const error = t.throws(() => m.run({ externalFunctions: { success, fail } }), isRuntimeError)
  t.true(error.message.includes('ValueError'))
  t.true(error.message.includes('second call fails'))
})

test('external function exception with finally', (t) => {
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
  const m = new Monty(code, { externalFunctions: ['fail'] })

  const fail = () => {
    const error = new Error('error')
    error.name = 'ValueError'
    throw error
  }

  t.is(m.run({ externalFunctions: { fail } }), true)
})
