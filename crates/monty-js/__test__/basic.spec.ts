import test from 'ava'

import { Monty, MontySyntaxError } from '../wrapper'

// =============================================================================
// Constructor tests
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
  const error = t.throws(() => new Monty('def'), { instanceOf: MontySyntaxError })
  t.true(error?.message.includes('SyntaxError'))
})

// =============================================================================
// repr() tests
// =============================================================================

test('Monty repr() no inputs', (t) => {
  const m = new Monty('1 + 1')
  const repr = m.repr()
  t.true(repr.includes('Monty'))
  t.true(repr.includes('main.py'))
})

test('Monty repr() with inputs', (t) => {
  const m = new Monty('x', { inputs: ['x', 'y'] })
  const repr = m.repr()
  t.true(repr.includes('Monty'))
  t.true(repr.includes('inputs'))
})

test('Monty repr() with external functions', (t) => {
  const m = new Monty('foo()', { externalFunctions: ['foo'] })
  const repr = m.repr()
  t.true(repr.includes('externalFunctions'))
})

test('Monty repr() with inputs and external functions', (t) => {
  const m = new Monty('foo(x)', { inputs: ['x'], externalFunctions: ['foo'] })
  const repr = m.repr()
  t.true(repr.includes('inputs'))
  t.true(repr.includes('externalFunctions'))
})

// =============================================================================
// Simple expression tests
// =============================================================================

test('simple expression', (t) => {
  const m = new Monty('1 + 2')
  t.is(m.run(), 3)
})

test('arithmetic', (t) => {
  const m = new Monty('10 * 5 - 3')
  t.is(m.run(), 47)
})

test('string concatenation', (t) => {
  const m = new Monty('"hello" + " " + "world"')
  t.is(m.run(), 'hello world')
})

// =============================================================================
// Multiple runs tests
// =============================================================================

test('multiple runs same instance', (t) => {
  const m = new Monty('x * 2', { inputs: ['x'] })
  t.is(m.run({ inputs: { x: 5 } }), 10)
  t.is(m.run({ inputs: { x: 10 } }), 20)
  t.is(m.run({ inputs: { x: -3 } }), -6)
})

test('run multiple times no inputs', (t) => {
  const m = new Monty('1 + 2')
  t.is(m.run(), 3)
  t.is(m.run(), 3)
  t.is(m.run(), 3)
})

// =============================================================================
// Multiline code tests
// =============================================================================

test('multiline code', (t) => {
  const code = `
x = 1
y = 2
x + y
`
  const m = new Monty(code)
  t.is(m.run(), 3)
})

test('function definition and call', (t) => {
  const code = `
def add(a, b):
    return a + b

add(3, 4)
`
  const m = new Monty(code)
  t.is(m.run(), 7)
})
