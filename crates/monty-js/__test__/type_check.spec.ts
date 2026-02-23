import test from 'ava'

import { Monty, MontyTypingError } from '../wrapper'

// =============================================================================
// typeCheck() tests
// =============================================================================

test('type check no errors', (t) => {
  const m = new Monty('x = 1')
  t.notThrows(() => m.typeCheck())
})

test('type check with errors', (t) => {
  const m = new Monty('"hello" + 1')
  const error = t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  t.true(error.message.includes('unsupported-operator'))
})

test('type check function return type', (t) => {
  const code = `
def foo() -> int:
    return "not an int"
`
  const m = new Monty(code)
  const error = t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  t.true(error.message.includes('invalid-return-type'))
})

test('type check undefined variable', (t) => {
  const m = new Monty('print(undefined_var)')
  const error = t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  t.true(error.message.includes('unresolved-reference'))
})

test('type check valid function', (t) => {
  const code = `
def add(a: int, b: int) -> int:
    return a + b

add(1, 2)
`
  const m = new Monty(code)
  t.notThrows(() => m.typeCheck())
})

test('type check with prefix code', (t) => {
  const m = new Monty('result = x + 1')
  // Without prefix, x is undefined
  t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  // With prefix declaring x as a variable, it should pass
  t.notThrows(() => m.typeCheck('x = 0'))
})

// =============================================================================
// Constructor type_check parameter tests
// =============================================================================

test('constructor type check default false', (t) => {
  // This should NOT raise during construction (typeCheck=false is default)
  const m = new Monty('"hello" + 1')
  // But we can still call typeCheck() manually later
  t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
})

test('constructor type check explicit true', (t) => {
  t.throws(() => new Monty('"hello" + 1', { typeCheck: true }), { instanceOf: MontyTypingError })
})

test('constructor type check explicit false', (t) => {
  // This should NOT raise during construction
  const m = new Monty('"hello" + 1', { typeCheck: false })
  // But we can still call typeCheck() manually later
  t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
})

test('constructor default allows run with inputs', (t) => {
  // Code with undefined variable - type checking would fail
  const m = new Monty('x + 1', { inputs: ['x'] })
  // But runtime works fine with the input provided
  const result = m.run({ inputs: { x: 5 } })
  t.is(result, 6)
})

test('constructor type check prefix code', (t) => {
  // Without prefix, this would fail type checking (x is undefined)
  // Use assignment to define x, not just type annotation
  t.notThrows(() => new Monty('result = x + 1', { typeCheck: true, typeCheckPrefixCode: 'x = 0' }))
})

test('constructor type check prefix code with external function', (t) => {
  // Define fetch as a function that takes a string and returns a string
  const prefix = `
def fetch(url: str) -> str:
    return ''
`
  t.notThrows(
    () =>
      new Monty('result = fetch("https://example.com")', {
        externalFunctions: ['fetch'],
        typeCheck: true,
        typeCheckPrefixCode: prefix,
      }),
  )
})

test('constructor type check prefix code invalid', (t) => {
  // Prefix defines x as str, but code tries to use it with int addition
  t.throws(
    () =>
      new Monty('result: int = x + 1', {
        typeCheck: true,
        typeCheckPrefixCode: 'x = "hello"',
      }),
    { instanceOf: MontyTypingError },
  )
})

// =============================================================================
// MontyTypingError tests
// =============================================================================

test('monty typing error is monty error subclass', (t) => {
  const m = new Monty('"hello" + 1')
  const error = t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  t.true(error instanceof Error)
})

test('monty typing error displayDiagnostics', (t) => {
  const m = new Monty('"hello" + 1')
  const error = t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  // displayDiagnostics() returns rich diagnostics, display('msg') returns the raw message
  t.is(error.message, `TypeError: ${error.display('msg')}`)
})

test('monty typing error displayDiagnostics concise format', (t) => {
  const m = new Monty('"hello" + 1')
  const error = t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  const concise = error.displayDiagnostics('concise')
  t.true(concise.includes('error[unsupported-operator]'))
})

test('monty typing error inherits base display formats', (t) => {
  const m = new Monty('"hello" + 1')
  const error = t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  t.is(error.display('msg'), error.exception.message)
  t.true(error.display('type-msg').startsWith('TypeError:'))
})
