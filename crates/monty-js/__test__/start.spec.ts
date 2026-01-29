import test from 'ava'

import {
  Monty,
  MontySnapshot,
  MontyComplete,
  MontyRuntimeError,
  type ResourceLimits,
  type ResumeOptions,
} from '../wrapper'

// =============================================================================
// start() returns MontyComplete tests
// =============================================================================

test('start no external functions returns complete', (t) => {
  const m = new Monty('1 + 2')
  const result = m.start()
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 3)
})

test('start returns complete for various types', (t) => {
  const testCases: Array<[string, unknown]> = [
    ['1', 1],
    ['"hello"', 'hello'],
    ['[1, 2, 3]', [1, 2, 3]],
    ['None', null],
    ['True', true],
  ]

  for (const [code, expected] of testCases) {
    const m = new Monty(code)
    const result = m.start()
    t.true(result instanceof MontyComplete)
    t.deepEqual((result as MontyComplete).output, expected)
  }
})

// =============================================================================
// start() returns MontySnapshot tests
// =============================================================================

test('start with external function returns progress', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const result = m.start()
  t.true(result instanceof MontySnapshot)
  const snapshot = result as MontySnapshot
  t.is(snapshot.scriptName, 'main.py')
  t.is(snapshot.functionName, 'func')
  t.deepEqual(snapshot.args, [])
  t.deepEqual(snapshot.kwargs, {})
})

test('start custom script name', (t) => {
  const m = new Monty('func()', { scriptName: 'custom.py', externalFunctions: ['func'] })
  const result = m.start()
  t.true(result instanceof MontySnapshot)
  t.is((result as MontySnapshot).scriptName, 'custom.py')
})

test('start progress with args', (t) => {
  const m = new Monty('func(1, 2, 3)', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  t.is(snapshot.functionName, 'func')
  t.deepEqual(snapshot.args, [1, 2, 3])
  t.deepEqual(snapshot.kwargs, {})
})

test('start progress with kwargs', (t) => {
  const m = new Monty('func(a=1, b="two")', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  t.is(snapshot.functionName, 'func')
  t.deepEqual(snapshot.args, [])
  t.deepEqual(snapshot.kwargs, { a: 1, b: 'two' })
})

test('start progress with mixed args kwargs', (t) => {
  const m = new Monty('func(1, 2, x="hello", y=True)', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  t.is(snapshot.functionName, 'func')
  t.deepEqual(snapshot.args, [1, 2])
  t.deepEqual(snapshot.kwargs, { x: 'hello', y: true })
})

// =============================================================================
// resume() tests
// =============================================================================

test('progress resume returns complete', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  t.is(snapshot.functionName, 'func')
  t.deepEqual(snapshot.args, [])
  t.deepEqual(snapshot.kwargs, {})

  const result = snapshot.resume({ returnValue: 42 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 42)
})

test('resume with none', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const snapshot = m.start() as MontySnapshot

  const result = snapshot.resume({ returnValue: null })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, null)
})

test('resume complex return value', (t) => {
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

// =============================================================================
// Multiple external function calls tests
// =============================================================================

test('multiple external calls', (t) => {
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

test('chain of external calls', (t) => {
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

// =============================================================================
// start() with options tests
// =============================================================================

test('start with inputs', (t) => {
  const m = new Monty('process(x)', { inputs: ['x'], externalFunctions: ['process'] })
  const progress = m.start({ inputs: { x: 100 } })
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  t.is(snapshot.functionName, 'process')
  t.deepEqual(snapshot.args, [100])
})

test('start with limits', (t) => {
  const m = new Monty('1 + 2')
  const limits: ResourceLimits = { maxAllocations: 1000 }
  const result = m.start({ limits })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 3)
})

// =============================================================================
// resume() cannot be called twice tests
// =============================================================================

test('resume cannot be called twice', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot

  // First resume succeeds
  snapshot.resume({ returnValue: 1 })

  // Second resume should fail
  const error = t.throws(() => snapshot.resume({ returnValue: 2 }))
  t.true(error?.message.includes('already'))
})

// =============================================================================
// resume() with exception tests
// =============================================================================

test('resume with exception caught', (t) => {
  const code = `
try:
    result = external_func()
except ValueError:
    caught = True
caught
`
  const m = new Monty(code, { externalFunctions: ['external_func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot

  // Resume with an exception using keyword argument
  const result = snapshot.resume({ exception: { type: 'ValueError', message: 'test error' } })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, true)
})

test('resume exception propagates uncaught', (t) => {
  const m = new Monty('external_func()', { externalFunctions: ['external_func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot

  // Resume with an exception that won't be caught - wrapped in MontyRuntimeError
  const error = t.throws(() => snapshot.resume({ exception: { type: 'ValueError', message: 'uncaught error' } }), {
    instanceOf: MontyRuntimeError,
  })
  t.true(error.message.includes('ValueError'))
  t.true(error.message.includes('uncaught error'))
})

test('resume exception in nested try', (t) => {
  const code = `
outer_caught = False
finally_ran = False
try:
    try:
        external_func()
    except TypeError:
        pass  # Won't catch ValueError
    finally:
        finally_ran = True
except ValueError:
    outer_caught = True
(outer_caught, finally_ran)
`
  const m = new Monty(code, { externalFunctions: ['external_func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot

  const result = snapshot.resume({ exception: { type: 'ValueError', message: 'propagates to outer' } })
  t.true(result instanceof MontyComplete)
  const output = (result as MontyComplete).output
  t.true(Array.isArray(output))
  t.is(output[0], true) // outer_caught
  t.is(output[1], true) // finally_ran
})

// =============================================================================
// Invalid resume args tests
// =============================================================================

test('invalid resume args', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot

  // Neither provided
  const error = t.throws(() => snapshot.resume({} as ResumeOptions))
  t.true(error?.message.includes('returnValue or exception'))
})

// =============================================================================
// Monty instance reuse tests
// =============================================================================

test('start can reuse monty instance', (t) => {
  const m = new Monty('func(x)', { inputs: ['x'], externalFunctions: ['func'] })

  // First run
  const progress1 = m.start({ inputs: { x: 1 } })
  t.true(progress1 instanceof MontySnapshot)
  t.deepEqual((progress1 as MontySnapshot).args, [1])
  const result1 = (progress1 as MontySnapshot).resume({ returnValue: 10 })
  t.true(result1 instanceof MontyComplete)
  t.is((result1 as MontyComplete).output, 10)

  // Second run with different input
  const progress2 = m.start({ inputs: { x: 2 } })
  t.true(progress2 instanceof MontySnapshot)
  t.deepEqual((progress2 as MontySnapshot).args, [2])
  const result2 = (progress2 as MontySnapshot).resume({ returnValue: 20 })
  t.true(result2 instanceof MontyComplete)
  t.is((result2 as MontyComplete).output, 20)
})

// =============================================================================
// repr() tests
// =============================================================================

test('progress repr', (t) => {
  const m = new Monty('func(1, x=2)', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const repr = (progress as MontySnapshot).repr()
  t.true(repr.includes('MontySnapshot'))
  t.true(repr.includes('func'))
})

test('complete repr', (t) => {
  const m = new Monty('42')
  const result = m.start()
  t.true(result instanceof MontyComplete)
  const repr = (result as MontyComplete).repr()
  t.true(repr.includes('MontyComplete'))
})
