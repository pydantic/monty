import test from 'ava'

import { Monty, type MontyOptions, type RunOptions, type ResourceLimits } from '../wrapper'
import { Buffer } from 'node:buffer'

// =============================================================================
// Monty class constructor tests
// =============================================================================

test('Monty constructor with default options', (t) => {
  const m = new Monty('1 + 2')
  t.is(m.scriptName, 'main.py')
  t.deepEqual(m.inputs, [])
  t.deepEqual(m.externalFunctions, [])
})

test('Monty constructor with custom script name', (t) => {
  const m = new Monty('1 + 2', { scriptName: 'test.py' })
  t.is(m.scriptName, 'test.py')
})

test('Monty constructor with inputs', (t) => {
  const m = new Monty('x + y', { inputs: ['x', 'y'] })
  t.deepEqual(m.inputs, ['x', 'y'])
})

test('Monty constructor with external functions', (t) => {
  const m = new Monty('foo()', { externalFunctions: ['foo'] })
  t.deepEqual(m.externalFunctions, ['foo'])
})

test('Monty constructor with syntax error', (t) => {
  const error = t.throws(() => new Monty('def'))
  t.true(error?.message.includes('SyntaxError'))
})

test('Monty repr()', (t) => {
  const m = new Monty('1 + 2', { scriptName: 'test.py', inputs: ['x'] })
  const repr = m.repr()
  t.true(repr.includes('Monty'))
  t.true(repr.includes('test.py'))
  t.true(repr.includes('inputs'))
})

// =============================================================================
// Monty.run() tests
// =============================================================================

test('Monty.run() simple expression', (t) => {
  const m = new Monty('1 + 2')
  const result = m.run()
  t.is(result, 3)
})

test('Monty.run() with string result', (t) => {
  const m = new Monty('"hello"')
  const result = m.run()
  t.is(result, 'hello')
})

test('Monty.run() with list result', (t) => {
  const m = new Monty('[1, 2, 3]')
  const result = m.run()
  t.deepEqual(result, [1, 2, 3])
})

test('Monty.run() with dict result', (t) => {
  const m = new Monty('{"a": 1, "b": 2}')
  const result = m.run()
  t.deepEqual(result, { a: 1, b: 2 })
})

test('Monty.run() with None result', (t) => {
  const m = new Monty('None')
  const result = m.run()
  t.is(result, null)
})

test('Monty.run() with bool result', (t) => {
  const m = new Monty('True')
  const result = m.run()
  t.is(result, true)
})

test('Monty.run() with float result', (t) => {
  const m = new Monty('3.14')
  const result = m.run()
  t.is(result, 3.14)
})

test('Monty.run() multiple times (reuse)', (t) => {
  const m = new Monty('1 + 2')
  t.is(m.run(), 3)
  t.is(m.run(), 3)
  t.is(m.run(), 3)
})

// =============================================================================
// Monty.run() with inputs tests
// =============================================================================

test('Monty.run() with inputs', (t) => {
  const options: MontyOptions = { inputs: ['x', 'y'] }
  const m = new Monty('x + y', options)

  const runOptions: RunOptions = { inputs: { x: 10, y: 20 } }
  const result = m.run(runOptions)
  t.is(result, 30)
})

test('Monty.run() with different inputs on reuse', (t) => {
  const m = new Monty('x * 2', { inputs: ['x'] })
  t.is(m.run({ inputs: { x: 5 } }), 10)
  t.is(m.run({ inputs: { x: 7 } }), 14)
})

test('Monty.run() with missing input', (t) => {
  const m = new Monty('x + y', { inputs: ['x', 'y'] })
  const error = t.throws(() => m.run({ inputs: { x: 10 } }))
  t.true(error?.message.includes('Missing required input'))
})

test('Monty.run() inputs not declared but provided', (t) => {
  const m = new Monty('1 + 2')
  const error = t.throws(() => m.run({ inputs: { x: 10 } }))
  t.true(error?.message.includes('No input variables declared'))
})

test('Monty.run() with complex input types', (t) => {
  const m = new Monty('len(items)', { inputs: ['items'] })
  const result = m.run({ inputs: { items: [1, 2, 3, 4, 5] } })
  t.is(result, 5)
})

// =============================================================================
// Monty.run() with resource limits tests
// =============================================================================

test('Monty.run() with resource limits', (t) => {
  const m = new Monty('1 + 2')
  const limits: ResourceLimits = { maxRecursionDepth: 100 }
  const result = m.run({ limits })
  t.is(result, 3)
})

test('Monty.run() exceeds recursion limit', (t) => {
  const code = `
def recurse(n):
    return recurse(n + 1)
recurse(0)
`
  const m = new Monty(code)
  const limits: ResourceLimits = { maxRecursionDepth: 10 }
  const error = t.throws(() => m.run({ limits }))
  t.true(error?.message.includes('RecursionError'))
})

test('Monty.run() with time limit', (t) => {
  // Use recursion instead of while loop since while loops aren't supported yet
  const code = `
def infinite(n):
    return infinite(n + 1)
infinite(0)
`
  const m = new Monty(code)
  const limits: ResourceLimits = { maxDurationSecs: 0.1 }
  const error = t.throws(() => m.run({ limits }))
  // May hit time limit or recursion limit
  t.true(
    error?.message.includes('TimeoutError') ||
      error?.message.includes('timed out') ||
      error?.message.includes('RecursionError'),
  )
})

// =============================================================================
// Monty.typeCheck() tests
// =============================================================================

test('Monty.typeCheck() passes for valid code', (t) => {
  const m = new Monty('x: int = 1')
  t.notThrows(() => m.typeCheck())
})

test('Monty.typeCheck() with type check enabled at construction', (t) => {
  // This should pass type checking
  t.notThrows(() => new Monty('x: int = 1', { typeCheck: true }))
})

// =============================================================================
// Monty.dump() and Monty.load() tests
// =============================================================================

test('Monty.dump() and Monty.load() roundtrip', (t) => {
  const original = new Monty('x + y', { inputs: ['x', 'y'], scriptName: 'test.py' })

  const bytes = original.dump()
  t.true(bytes instanceof Buffer)
  t.true(bytes.length > 0)

  const loaded = Monty.load(bytes)
  t.is(loaded.scriptName, 'test.py')
  t.deepEqual(loaded.inputs, ['x', 'y'])

  // Run the loaded instance
  const result = loaded.run({ inputs: { x: 3, y: 4 } })
  t.is(result, 7)
})

test('Monty.dump() produces same result on multiple calls', (t) => {
  const m = new Monty('1 + 2')
  const bytes1 = m.dump()
  const bytes2 = m.dump()
  t.deepEqual(bytes1, bytes2)
})

// =============================================================================
// Error handling tests
// =============================================================================

test('runtime error includes traceback', (t) => {
  const code = `
def foo():
    raise ValueError("test error")

def bar():
    foo()

bar()
`
  const m = new Monty(code)
  const error = t.throws(() => m.run())
  t.true(error?.message.includes('ValueError: test error'))
  t.true(error?.message.includes('Traceback'))
  t.true(error?.message.includes('foo'))
  t.true(error?.message.includes('bar'))
})

test('zero division error', (t) => {
  const m = new Monty('1 / 0')
  const error = t.throws(() => m.run())
  t.true(error?.message.includes('ZeroDivisionError'))
})

test('name error', (t) => {
  const m = new Monty('undefined_variable')
  const error = t.throws(() => m.run())
  t.true(error?.message.includes('NameError'))
})

test('index error', (t) => {
  const m = new Monty('[1, 2, 3][10]')
  const error = t.throws(() => m.run())
  t.true(error?.message.includes('IndexError'))
})

test('key error', (t) => {
  const m = new Monty('{"a": 1}["b"]')
  const error = t.throws(() => m.run())
  t.true(error?.message.includes('KeyError'))
})

// =============================================================================
// Type conversion tests
// =============================================================================

test('tuple result', (t) => {
  const m = new Monty('(1, 2, 3)')
  const result = m.run()
  // Tuples are returned as objects with _type: "Tuple"
  t.is(result._type, 'Tuple')
  t.deepEqual(result._value, [1, 2, 3])
})

test('set result', (t) => {
  const m = new Monty('{1, 2, 3}')
  const result = m.run()
  t.is(result._type, 'Set')
  // Sets may not preserve order
  t.is(result._value.length, 3)
})

test('nested data structures', (t) => {
  const m = new Monty('{"list": [1, 2], "nested": {"a": 1}}')
  const result = m.run()
  t.deepEqual(result, { list: [1, 2], nested: { a: 1 } })
})

test('bytes result', (t) => {
  const m = new Monty('b"hello"')
  const result = m.run()
  t.is(result._type, 'Bytes')
  // ASCII values for "hello"
  t.deepEqual(result._value, [104, 101, 108, 108, 111])
})
