import test from 'ava'

import {
  Monty,
  MontySnapshot,
  MontyComplete,
  type MontyOptions,
  type RunOptions,
  type ResourceLimits,
  type ResumeOptions,
} from '../wrapper'
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
  // Dicts are returned as native JS Map (preserves key types and insertion order)
  t.true(result instanceof Map)
  t.is(result.get('a'), 1)
  t.is(result.get('b'), 2)
  t.is(result.size, 2)
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
  // Tuples are returned as arrays with a __tuple__ marker property
  t.true(Array.isArray(result))
  t.deepEqual([...result], [1, 2, 3])
  t.is(result.__tuple__, true)
})

test('set result', (t) => {
  const m = new Monty('{1, 2, 3}')
  const result = m.run()
  t.deepEqual(result, new Set([1, 2, 3]))
})

test('nested data structures', (t) => {
  const m = new Monty('{"list": [1, 2], "nested": {"a": 1}}')
  const result = m.run()
  // Dicts are returned as native JS Map
  t.true(result instanceof Map)
  t.deepEqual(result.get('list'), [1, 2])
  const nested = result.get('nested')
  t.true(nested instanceof Map)
  t.is(nested.get('a'), 1)
})

test('bytes result', (t) => {
  const m = new Monty('b"hello"')
  const result = m.run()
  // Bytes are returned as Buffer (Node.js native)
  t.true(Buffer.isBuffer(result))
  // ASCII values for "hello"
  t.deepEqual([...result], [104, 101, 108, 108, 111])
})

test('frozenset result', (t) => {
  const m = new Monty('frozenset([1, 2, 3])')
  const result = m.run()
  // FrozenSet is returned as a native JS Set (no frozen equivalent in JS)
  t.true(result instanceof Set)
  t.deepEqual(result, new Set([1, 2, 3]))
})

test('nested set in list', (t) => {
  const m = new Monty('[{1, 2}, {3, 4}]')
  const result = m.run()
  t.true(Array.isArray(result))
  t.is(result.length, 2)
  t.true(result[0] instanceof Set)
  t.true(result[1] instanceof Set)
  t.deepEqual(result[0], new Set([1, 2]))
  t.deepEqual(result[1], new Set([3, 4]))
})

test('nested bytes in dict', (t) => {
  const m = new Monty('{"data": b"abc"}')
  const result = m.run()
  // Dicts are returned as native JS Map
  t.true(result instanceof Map)
  const data = result.get('data')
  t.true(Buffer.isBuffer(data))
  t.deepEqual([...data], [97, 98, 99])
})

test('tuple containing set', (t) => {
  const m = new Monty('({1, 2}, "hello")')
  const result = m.run()
  t.true(Array.isArray(result))
  t.is(result.__tuple__, true)
  t.true(result[0] instanceof Set)
  t.deepEqual(result[0], new Set([1, 2]))
  t.is(result[1], 'hello')
})

// =============================================================================
// Monty.start() and iterative execution tests
// =============================================================================

test('start() returns MontyComplete for code without external functions', (t) => {
  const m = new Monty('1 + 2')
  const result = m.start()
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 3)
})

test('start() returns MontySnapshot for code with external function call', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const result = m.start()
  t.true(result instanceof MontySnapshot)
  const snapshot = result as MontySnapshot
  t.is(snapshot.scriptName, 'main.py')
  t.is(snapshot.functionName, 'func')
  t.deepEqual(snapshot.args, [])
  t.deepEqual(snapshot.kwargs, {})
})

test('start() with custom script name', (t) => {
  const m = new Monty('func()', { scriptName: 'custom.py', externalFunctions: ['func'] })
  const result = m.start()
  t.true(result instanceof MontySnapshot)
  t.is((result as MontySnapshot).scriptName, 'custom.py')
})

test('start() captures function arguments', (t) => {
  const m = new Monty('func(1, 2, 3)', { externalFunctions: ['func'] })
  const result = m.start()
  t.true(result instanceof MontySnapshot)
  const snapshot = result as MontySnapshot
  t.deepEqual(snapshot.args, [1, 2, 3])
})

test('start() captures keyword arguments', (t) => {
  const m = new Monty('func(a=1, b="two")', { externalFunctions: ['func'] })
  const result = m.start()
  t.true(result instanceof MontySnapshot)
  const snapshot = result as MontySnapshot
  t.deepEqual(snapshot.args, [])
  t.deepEqual(snapshot.kwargs, { a: 1, b: 'two' })
})

test('start() captures mixed positional and keyword arguments', (t) => {
  const m = new Monty('func(1, 2, x="hello", y=True)', { externalFunctions: ['func'] })
  const result = m.start()
  t.true(result instanceof MontySnapshot)
  const snapshot = result as MontySnapshot
  t.deepEqual(snapshot.args, [1, 2])
  t.deepEqual(snapshot.kwargs, { x: 'hello', y: true })
})

test('resume() with return value completes execution', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot
  t.true(snapshot instanceof MontySnapshot)

  const result = snapshot.resume({ returnValue: 42 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 42)
})

test('resume() with None/null return value', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  const result = snapshot.resume({ returnValue: null })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, null)
})

test('resume() with complex return value', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  const complexValue = { a: [1, 2, 3], b: { nested: true } }
  const result = snapshot.resume({ returnValue: complexValue })
  t.true(result instanceof MontyComplete)
  // JS objects become Maps in Python (and come back as Maps)
  const output = (result as MontyComplete).output as Map<string, unknown>
  t.true(output instanceof Map)
  t.deepEqual(output.get('a'), [1, 2, 3])
  const nestedMap = output.get('b') as Map<string, unknown>
  t.true(nestedMap instanceof Map)
  t.is(nestedMap.get('nested'), true)
})

test('multiple external function calls in sequence', (t) => {
  const m = new Monty('a() + b()', { externalFunctions: ['a', 'b'] })

  // First call
  let progress = m.start()
  t.true(progress instanceof MontySnapshot)
  t.is((progress as MontySnapshot).functionName, 'a')

  // Resume with first return value
  progress = (progress as MontySnapshot).resume({ returnValue: 10 })
  t.true(progress instanceof MontySnapshot)
  t.is((progress as MontySnapshot).functionName, 'b')

  // Resume with second return value
  const result = (progress as MontySnapshot).resume({ returnValue: 5 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 15)
})

test('chain of external calls with same function', (t) => {
  const m = new Monty('c() + c() + c()', { externalFunctions: ['c'] })

  let callCount = 0
  let progress: MontySnapshot | MontyComplete = m.start()

  while (progress instanceof MontySnapshot) {
    t.is(progress.functionName, 'c')
    callCount += 1
    progress = progress.resume({ returnValue: callCount })
  }

  t.true(progress instanceof MontyComplete)
  t.is((progress as MontyComplete).output, 6) // 1 + 2 + 3
  t.is(callCount, 3)
})

test('start() with inputs', (t) => {
  const m = new Monty('process(x)', { inputs: ['x'], externalFunctions: ['process'] })
  const progress = m.start({ inputs: { x: 100 } })
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  t.is(snapshot.functionName, 'process')
  t.deepEqual(snapshot.args, [100])
})

test('start() with resource limits', (t) => {
  const m = new Monty('1 + 2')
  const limits: ResourceLimits = { maxAllocations: 1000 }
  const result = m.start({ limits })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 3)
})

test('resume() cannot be called twice on same snapshot', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  // First resume succeeds
  snapshot.resume({ returnValue: 1 })

  // Second resume should fail
  const error = t.throws(() => snapshot.resume({ returnValue: 2 }))
  t.true(error?.message.includes('already been resumed'))
})

test('resume() with exception that is caught', (t) => {
  const code = `
try:
    result = external_func()
except ValueError:
    caught = True
caught
`
  const m = new Monty(code, { externalFunctions: ['external_func'] })
  const snapshot = m.start() as MontySnapshot

  const result = snapshot.resume({ exception: { type: 'ValueError', message: 'test error' } })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, true)
})

test('resume() with uncaught exception propagates', (t) => {
  const m = new Monty('external_func()', { externalFunctions: ['external_func'] })
  const snapshot = m.start() as MontySnapshot

  const error = t.throws(() => snapshot.resume({ exception: { type: 'ValueError', message: 'uncaught error' } }))
  t.true(error?.message.includes('ValueError'))
  t.true(error?.message.includes('uncaught error'))
})

test('resume() requires exactly one of returnValue or exception', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  // Neither provided
  const error1 = t.throws(() => snapshot.resume({} as ResumeOptions))
  t.true(error1?.message.includes('returnValue or exception'))
})

test('Monty instance can be reused after start()', (t) => {
  const m = new Monty('func(x)', { inputs: ['x'], externalFunctions: ['func'] })

  // First run
  const progress1 = m.start({ inputs: { x: 1 } }) as MontySnapshot
  t.deepEqual(progress1.args, [1])
  const result1 = progress1.resume({ returnValue: 10 })
  t.is((result1 as MontyComplete).output, 10)

  // Second run with different input
  const progress2 = m.start({ inputs: { x: 2 } }) as MontySnapshot
  t.deepEqual(progress2.args, [2])
  const result2 = progress2.resume({ returnValue: 20 })
  t.is((result2 as MontyComplete).output, 20)
})

test('MontyComplete.repr()', (t) => {
  const m = new Monty('42')
  const result = m.start() as MontyComplete
  const repr = result.repr()
  t.true(repr.includes('MontyComplete'))
})

test('MontySnapshot.repr()', (t) => {
  const m = new Monty('func(1, x=2)', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot
  const repr = snapshot.repr()
  t.true(repr.includes('MontySnapshot'))
  t.true(repr.includes('func'))
})

// =============================================================================
// MontySnapshot serialization tests
// =============================================================================

test('MontySnapshot.dump() and MontySnapshot.load() roundtrip', (t) => {
  const m = new Monty('func(1, 2)', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  const data = snapshot.dump()
  t.true(data instanceof Buffer)
  t.true(data.length > 0)

  const loaded = MontySnapshot.load(data)
  t.is(loaded.functionName, 'func')
  t.deepEqual(loaded.args, [1, 2])
  t.deepEqual(loaded.kwargs, {})

  const result = loaded.resume({ returnValue: 100 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 100)
})

test('MontySnapshot.dump() preserves script name', (t) => {
  const m = new Monty('func()', { scriptName: 'test.py', externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  const data = snapshot.dump()
  const loaded = MontySnapshot.load(data)
  t.is(loaded.scriptName, 'test.py')
})

test('MontySnapshot.dump() preserves kwargs', (t) => {
  const m = new Monty('func(a=1, b="hello")', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  const data = snapshot.dump()
  const loaded = MontySnapshot.load(data)
  t.deepEqual(loaded.kwargs, { a: 1, b: 'hello' })
})

test('MontySnapshot.dump() after resume fails', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  snapshot.resume({ returnValue: 1 })

  const error = t.throws(() => snapshot.dump())
  t.true(error?.message.includes('already been resumed'))
})

test('MontySnapshot serialization with multiple calls', (t) => {
  const m = new Monty('a() + b()', { externalFunctions: ['a', 'b'] })

  // First call
  let progress = m.start() as MontySnapshot
  t.is(progress.functionName, 'a')

  // Dump and load
  const data1 = progress.dump()
  progress = MontySnapshot.load(data1)

  // Resume with first return value
  progress = progress.resume({ returnValue: 10 }) as MontySnapshot
  t.is(progress.functionName, 'b')

  // Dump and load again
  const data2 = progress.dump()
  progress = MontySnapshot.load(data2)

  // Resume with second return value
  const result = progress.resume({ returnValue: 5 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 15)
})
