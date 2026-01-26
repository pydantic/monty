import test from 'ava'

import { Monty, MontySnapshot, MontyComplete, type ResourceLimits } from '../wrapper'
import { Buffer } from 'node:buffer'

// =============================================================================
// Monty dump/load tests
// =============================================================================

test('monty dump load roundtrip', (t) => {
  const m = new Monty('x + 1', { inputs: ['x'] })
  const data = m.dump()

  t.true(data instanceof Buffer)
  t.true(data.length > 0)

  const m2 = Monty.load(data)
  t.is(m2.run({ inputs: { x: 41 } }), 42)
})

test('monty dump load preserves script name', (t) => {
  const m = new Monty('1', { scriptName: 'custom.py' })
  const data = m.dump()

  const m2 = Monty.load(data)
  t.is(m2.scriptName, 'custom.py')
})

test('monty dump load preserves inputs', (t) => {
  const m = new Monty('x + y', { inputs: ['x', 'y'] })
  const data = m.dump()

  const m2 = Monty.load(data)
  t.deepEqual(m2.inputs, ['x', 'y'])
  t.is(m2.run({ inputs: { x: 1, y: 2 } }), 3)
})

test('monty dump load preserves external functions', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const data = m.dump()

  const m2 = Monty.load(data)
  t.deepEqual(m2.externalFunctions, ['func'])
})

test('monty dump produces same result on multiple calls', (t) => {
  const m = new Monty('1 + 2')
  const bytes1 = m.dump()
  const bytes2 = m.dump()
  t.deepEqual(bytes1, bytes2)
})

test('monty dump load various outputs', (t) => {
  const testCases: Array<[string, unknown]> = [
    ['1 + 1', 2],
    ['"hello"', 'hello'],
    ['[1, 2, 3]', [1, 2, 3]],
    ['True', true],
    ['None', null],
  ]

  for (const [code, expected] of testCases) {
    const m = new Monty(code)
    const data = m.dump()
    const m2 = Monty.load(data)
    t.deepEqual(m2.run(), expected)
  }
})

// =============================================================================
// MontySnapshot dump/load tests
// =============================================================================

test('snapshot dump load roundtrip', (t) => {
  const m = new Monty('func(1, 2)', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)

  const data = (progress as MontySnapshot).dump()
  t.true(data instanceof Buffer)
  t.true(data.length > 0)

  const progress2 = MontySnapshot.load(data)
  t.is(progress2.functionName, 'func')
  t.deepEqual(progress2.args, [1, 2])
  t.deepEqual(progress2.kwargs, {})

  const result = progress2.resume({ returnValue: 100 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 100)
})

test('snapshot dump load preserves script name', (t) => {
  const m = new Monty('func()', { scriptName: 'test.py', externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)

  const data = (progress as MontySnapshot).dump()
  const progress2 = MontySnapshot.load(data)
  t.is(progress2.scriptName, 'test.py')
})

test('snapshot dump load with kwargs', (t) => {
  const m = new Monty('func(a=1, b="hello")', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)

  const data = (progress as MontySnapshot).dump()
  const progress2 = MontySnapshot.load(data)
  t.is(progress2.functionName, 'func')
  t.deepEqual(progress2.args, [])
  t.deepEqual(progress2.kwargs, { a: 1, b: 'hello' })
})

test('snapshot dump after resume fails', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot

  snapshot.resume({ returnValue: 1 })

  const error = t.throws(() => snapshot.dump())
  t.true(error?.message.includes('already been resumed'))
})

test('snapshot dump load multiple calls', (t) => {
  const m = new Monty('a() + b()', { externalFunctions: ['a', 'b'] })

  // First call
  let progress = m.start() as MontySnapshot
  t.is(progress.functionName, 'a')

  // Dump and load the state
  const data = progress.dump()
  progress = MontySnapshot.load(data)

  // Resume with first return value
  let progress3 = progress.resume({ returnValue: 10 }) as MontySnapshot
  t.is(progress3.functionName, 'b')

  // Dump and load again
  const data2 = progress3.dump()
  progress3 = MontySnapshot.load(data2)

  // Resume with second return value
  const result = progress3.resume({ returnValue: 5 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 15)
})

test('snapshot dump load with limits', (t) => {
  const m = new Monty('func()', { externalFunctions: ['func'] })
  const limits: ResourceLimits = { maxAllocations: 1000 }
  const progress = m.start({ limits })
  t.true(progress instanceof MontySnapshot)

  const data = (progress as MontySnapshot).dump()
  const progress2 = MontySnapshot.load(data)

  const result = progress2.resume({ returnValue: 99 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 99)
})
