import test from 'ava'

import { Monty } from '../wrapper'

// =============================================================================
// Single input tests
// =============================================================================

test('single input', (t) => {
  const m = new Monty('x', { inputs: ['x'] })
  t.is(m.run({ inputs: { x: 42 } }), 42)
})

test('multiple inputs', (t) => {
  const m = new Monty('x + y + z', { inputs: ['x', 'y', 'z'] })
  t.is(m.run({ inputs: { x: 1, y: 2, z: 3 } }), 6)
})

test('input used in expression', (t) => {
  const m = new Monty('x * 2 + y', { inputs: ['x', 'y'] })
  t.is(m.run({ inputs: { x: 5, y: 3 } }), 13)
})

test('input string', (t) => {
  const m = new Monty('greeting + " " + name', { inputs: ['greeting', 'name'] })
  t.is(m.run({ inputs: { greeting: 'Hello', name: 'World' } }), 'Hello World')
})

test('input list', (t) => {
  const m = new Monty('data[0] + data[1]', { inputs: ['data'] })
  t.is(m.run({ inputs: { data: [10, 20] } }), 30)
})

test('input dict', (t) => {
  const m = new Monty('config["a"] * config["b"]', { inputs: ['config'] })
  t.is(m.run({ inputs: { config: { a: 3, b: 4 } } }), 12)
})

// =============================================================================
// Missing input tests
// =============================================================================

test('missing input raises', (t) => {
  const m = new Monty('x + y', { inputs: ['x', 'y'] })
  const error = t.throws(() => m.run({ inputs: { x: 1 } }))
  t.true(error?.message.includes('Missing required input'))
})

test('all inputs missing raises', (t) => {
  const m = new Monty('x', { inputs: ['x'] })
  const error = t.throws(() => m.run())
  t.true(error?.message.includes('Missing required input'))
})

test('no inputs declared but provided raises', (t) => {
  const m = new Monty('1 + 1')
  const error = t.throws(() => m.run({ inputs: { x: 1 } }))
  t.true(error?.message.includes('No input variables declared'))
})

// =============================================================================
// Input order tests
// =============================================================================

test('inputs order independent', (t) => {
  const m = new Monty('a - b', { inputs: ['a', 'b'] })
  // Dict order shouldn't matter
  t.is(m.run({ inputs: { b: 3, a: 10 } }), 7)
})

// =============================================================================
// Function parameter shadowing tests
// =============================================================================

test('function param shadows input', (t) => {
  const code = `
def foo(x):
    return x + 1

foo(x * 2)
`
  const m = new Monty(code, { inputs: ['x'] })
  // x=5, so foo(x * 2) = foo(10), and inside foo, x is 10 (not 5), so returns 11
  t.is(m.run({ inputs: { x: 5 } }), 11)
})

test('function param shadows input multiple params', (t) => {
  const code = `
def add(x, y):
    return x + y

add(x * 10, y * 100)
`
  const m = new Monty(code, { inputs: ['x', 'y'] })
  // x=2, y=3, so add(20, 300) should return 320
  t.is(m.run({ inputs: { x: 2, y: 3 } }), 320)
})

test('input accessible outside shadowing function', (t) => {
  const code = `
def double(x):
    return x * 2

result = double(10) + x
result
`
  const m = new Monty(code, { inputs: ['x'] })
  // double(10) = 20, x (input) = 5, so result = 25
  t.is(m.run({ inputs: { x: 5 } }), 25)
})

test('function param shadows input with default', (t) => {
  const code = `
def foo(x=100):
    return x + 1

foo(x * 2)
`
  const m = new Monty(code, { inputs: ['x'] })
  // x=5, foo(10), inside foo x=10 (not 5 or 100), returns 11
  t.is(m.run({ inputs: { x: 5 } }), 11)
})

test('function uses input directly', (t) => {
  const code = `
def foo(y):
    return x + y

foo(10)
`
  const m = new Monty(code, { inputs: ['x'] })
  // x=5 (input), foo(10) with y=10, returns x + y = 5 + 10 = 15
  t.is(m.run({ inputs: { x: 5 } }), 15)
})

// =============================================================================
// Complex input types tests
// =============================================================================

test('complex input types', (t) => {
  const m = new Monty('len(items)', { inputs: ['items'] })
  t.is(m.run({ inputs: { items: [1, 2, 3, 4, 5] } }), 5)
})
