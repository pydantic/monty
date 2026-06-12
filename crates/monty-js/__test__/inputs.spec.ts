import test from 'ava'

import { MontyRuntimeError } from '../ts/index.js'
import { setupPool } from './helpers.js'

const { run } = setupPool(test)

// =============================================================================
// Single input tests
// =============================================================================

test('single input', async (t) => {
  t.is(await run('x', { inputs: { x: 42 } }), 42)
})

test('multiple inputs', async (t) => {
  t.is(await run('x + y + z', { inputs: { x: 1, y: 2, z: 3 } }), 6)
})

test('input used in expression', async (t) => {
  t.is(await run('x * 2 + y', { inputs: { x: 5, y: 3 } }), 13)
})

test('input string', async (t) => {
  t.is(await run('greeting + " " + name', { inputs: { greeting: 'Hello', name: 'World' } }), 'Hello World')
})

test('input list', async (t) => {
  t.is(await run('data[0] + data[1]', { inputs: { data: [10, 20] } }), 30)
})

test('input dict', async (t) => {
  t.is(await run('config["a"] * config["b"]', { inputs: { config: { a: 3, b: 4 } } }), 12)
})

// =============================================================================
// Missing input tests
// =============================================================================

test('missing input raises', async (t) => {
  const error = await t.throwsAsync(() => run('x + y', { inputs: { x: 1 } }), { instanceOf: MontyRuntimeError })
  t.is(error.message, "NameError: name 'y' is not defined")
})

test('all inputs missing raises', async (t) => {
  const error = await t.throwsAsync(() => run('x'), { instanceOf: MontyRuntimeError })
  t.is(error.message, "NameError: name 'x' is not defined")
})

test('unused inputs are allowed', async (t) => {
  // Inputs are no longer declared up front, so passing an input the code never
  // references is fine — it is simply bound and ignored.
  t.is(await run('1 + 1', { inputs: { x: 1 } }), 2)
})

// =============================================================================
// Input order tests
// =============================================================================

test('inputs order independent', async (t) => {
  // Dict order shouldn't matter
  t.is(await run('a - b', { inputs: { b: 3, a: 10 } }), 7)
})

// =============================================================================
// Function parameter shadowing tests
// =============================================================================

test('function param shadows input', async (t) => {
  const code = `
def foo(x):
    return x + 1

foo(x * 2)
`
  // x=5, so foo(x * 2) = foo(10), and inside foo, x is 10 (not 5), so returns 11
  t.is(await run(code, { inputs: { x: 5 } }), 11)
})

test('function param shadows input multiple params', async (t) => {
  const code = `
def add(x, y):
    return x + y

add(x * 10, y * 100)
`
  // x=2, y=3, so add(20, 300) should return 320
  t.is(await run(code, { inputs: { x: 2, y: 3 } }), 320)
})

test('input accessible outside shadowing function', async (t) => {
  const code = `
def double(x):
    return x * 2

result = double(10) + x
result
`
  // double(10) = 20, x (input) = 5, so result = 25
  t.is(await run(code, { inputs: { x: 5 } }), 25)
})

test('function param shadows input with default', async (t) => {
  const code = `
def foo(x=100):
    return x + 1

foo(x * 2)
`
  // x=5, foo(10), inside foo x=10 (not 5 or 100), returns 11
  t.is(await run(code, { inputs: { x: 5 } }), 11)
})

test('function uses input directly', async (t) => {
  const code = `
def foo(y):
    return x + y

foo(10)
`
  // x=5 (input), foo(10) with y=10, returns x + y = 5 + 10 = 15
  t.is(await run(code, { inputs: { x: 5 } }), 15)
})

// =============================================================================
// Complex input types tests
// =============================================================================

test('complex input types', async (t) => {
  t.is(await run('len(items)', { inputs: { items: [1, 2, 3, 4, 5] } }), 5)
})
