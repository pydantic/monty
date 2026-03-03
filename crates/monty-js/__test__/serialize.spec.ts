import test from 'ava'

import { Monty, MontySnapshot, MontyNameLookup, MontyComplete, type ResourceLimits } from '../wrapper'
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

test('monty dump load preserves code execution', (t) => {
  const m = new Monty('func()')
  const data = m.dump()

  const m2 = Monty.load(data)
  const progress = m2.start()
  t.true(progress instanceof MontySnapshot)
  t.is((progress as MontySnapshot).functionName, 'func')
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
  const m = new Monty('func(1, 2)')
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot

  t.is(snapshot.functionName, 'func')
  t.deepEqual(snapshot.args, [1, 2])
  t.deepEqual(snapshot.kwargs, {})

  const data = snapshot.dump()
  t.true(data instanceof Buffer)
  t.true(data.length > 0)

  const snapshot2 = MontySnapshot.load(data)
  t.is(snapshot2.functionName, 'func')
  t.deepEqual(snapshot2.args, [1, 2])
  t.deepEqual(snapshot2.kwargs, {})

  const result = snapshot2.resume({ returnValue: 100 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 100)
})

test('snapshot dump load preserves script name', (t) => {
  const m = new Monty('func()', { scriptName: 'test.py' })
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)

  const data = (progress as MontySnapshot).dump()
  const progress2 = MontySnapshot.load(data)
  t.is(progress2.scriptName, 'test.py')
})

test('snapshot dump load with kwargs', (t) => {
  const m = new Monty('func(a=1, b="hello")')
  const progress = m.start()
  t.true(progress instanceof MontySnapshot)

  const data = (progress as MontySnapshot).dump()
  const progress2 = MontySnapshot.load(data)
  t.is(progress2.functionName, 'func')
  t.deepEqual(progress2.args, [])
  t.deepEqual(progress2.kwargs, { a: 1, b: 'hello' })
})

test('snapshot dump after resume fails', (t) => {
  const m = new Monty('func()')
  const snapshot = m.start() as MontySnapshot

  snapshot.resume({ returnValue: 1 })

  const error = t.throws(() => snapshot.dump())
  t.true(error?.message.includes('already been resumed'))
})

test('snapshot dump load multiple calls', (t) => {
  const m = new Monty('a() + b()')

  // First call: a()
  let progress: MontySnapshot | MontyNameLookup | MontyComplete = m.start()
  t.true(progress instanceof MontySnapshot)
  let snapshot = progress as MontySnapshot
  t.is(snapshot.functionName, 'a')

  // Dump and load the state
  const data = snapshot.dump()
  snapshot = MontySnapshot.load(data)

  // Resume with first return value — triggers b()
  progress = snapshot.resume({ returnValue: 10 })
  t.true(progress instanceof MontySnapshot)
  let snapshot2 = progress as MontySnapshot
  t.is(snapshot2.functionName, 'b')

  // Dump and load again
  const data2 = snapshot2.dump()
  snapshot2 = MontySnapshot.load(data2)

  // Resume with second return value
  const result = snapshot2.resume({ returnValue: 5 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 15)
})

test('snapshot dump load with limits', (t) => {
  const m = new Monty('func()')
  const limits: ResourceLimits = { maxAllocations: 1000 }
  const progress = m.start({ limits })
  t.true(progress instanceof MontySnapshot)

  const data = (progress as MontySnapshot).dump()
  const progress2 = MontySnapshot.load(data)

  const result = progress2.resume({ returnValue: 99 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 99)
})

// =============================================================================
// MontyNameLookup dump/load tests
// =============================================================================

test('name lookup dump load roundtrip', (t) => {
  const m = new Monty('x = foo; x')
  const lookup = m.start()
  t.true(lookup instanceof MontyNameLookup)

  const data = (lookup as MontyNameLookup).dump()
  t.true(data instanceof Buffer)
  t.true(data.length > 0)

  const lookup2 = MontyNameLookup.load(data)
  t.is(lookup2.variableName, 'foo')
  t.is(lookup2.scriptName, 'main.py')

  const result = lookup2.resume({ value: 42 })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, 42)
})

test('name lookup dump after resume fails', (t) => {
  const m = new Monty('x = foo; x')
  const lookup = m.start() as MontyNameLookup

  lookup.resume({ value: 42 })

  const error = t.throws(() => lookup.dump())
  t.true(error?.message.includes('already been resumed'))
})
