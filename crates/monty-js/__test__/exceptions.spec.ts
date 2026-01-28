import test from 'ava'

import type { ErrorConstructor } from 'ava'

import { Monty, MontyError, MontySyntaxError, MontyRuntimeError, MontyTypingError } from '../wrapper'

// Helper for asserting MontyRuntimeError, private constructor requires the awkward cast via any
// but it works fine at runtime
export const isRuntimeError = { instanceOf: MontyRuntimeError as any as ErrorConstructor<MontyRuntimeError> }

// =============================================================================
// MontyRuntimeError tests
// =============================================================================

test('zero division error', (t) => {
  const m = new Monty('1 / 0')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'ZeroDivisionError: division by zero')
})

test('value error', (t) => {
  const m = new Monty('raise ValueError("bad value")')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'ValueError: bad value')
})

test('type error', (t) => {
  const m = new Monty("'string' + 1")
  const error = t.throws(() => m.run(), isRuntimeError)
  t.true(error.message.includes('TypeError'))
})

test('index error', (t) => {
  const m = new Monty('[1, 2, 3][10]')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'IndexError: list index out of range')
})

test('key error', (t) => {
  const m = new Monty('{"a": 1}["b"]')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'KeyError: b')
})

test('attribute error', (t) => {
  const m = new Monty('raise AttributeError("no such attr")')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'AttributeError: no such attr')
})

test('name error', (t) => {
  const m = new Monty('undefined_variable')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, "NameError: name 'undefined_variable' is not defined")
})

test('assertion error', (t) => {
  const m = new Monty('assert False')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.true(error.message.includes('AssertionError'))
})

test('assertion error with message', (t) => {
  const m = new Monty('assert False, "custom message"')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'AssertionError: custom message')
})

test('runtime error', (t) => {
  const m = new Monty('raise RuntimeError("runtime error")')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'RuntimeError: runtime error')
})

test('not implemented error', (t) => {
  const m = new Monty('raise NotImplementedError("not implemented")')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'NotImplementedError: not implemented')
})

// =============================================================================
// MontySyntaxError tests
// =============================================================================

test('syntax error on init', (t) => {
  const error = t.throws(() => new Monty('def'), { instanceOf: MontySyntaxError })
  t.true(error.message.includes('SyntaxError'))
})

test('syntax error unclosed paren', (t) => {
  const error = t.throws(() => new Monty('print(1'), { instanceOf: MontySyntaxError })
  t.true(error.message.includes('SyntaxError'))
})

test('syntax error invalid syntax', (t) => {
  const error = t.throws(() => new Monty('x = = 1'), { instanceOf: MontySyntaxError })
  t.true(error.message.includes('SyntaxError'))
})

// =============================================================================
// Catching with base class tests
// =============================================================================

test('catch with base class', (t) => {
  const m = new Monty('1 / 0')
  try {
    m.run()
    t.fail('Should have thrown')
  } catch (e) {
    t.true(e instanceof MontyError)
  }
})

test('catch syntax error with base class', (t) => {
  try {
    new Monty('def')
  } catch (e) {
    t.true(e instanceof MontyError)
  }
})

// =============================================================================
// Exception handling within Monty tests
// =============================================================================

test('raise caught exception', (t) => {
  const code = `
try:
    1 / 0
except ZeroDivisionError as e:
    result = 'caught'
result
`
  const m = new Monty(code)
  t.is(m.run(), 'caught')
})

test('exception in function', (t) => {
  const code = `
def fail():
    raise ValueError('from function')

fail()
`
  const m = new Monty(code)
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'ValueError: from function')
})

// =============================================================================
// Display and str methods tests
// =============================================================================

test('display traceback', (t) => {
  const m = new Monty('1 / 0')
  const error = t.throws(() => m.run(), isRuntimeError)
  const display = error.display('traceback')
  t.true(display.includes('Traceback (most recent call last):'))
  t.true(display.includes('ZeroDivisionError'))
})

test('display type msg', (t) => {
  const m = new Monty('raise ValueError("test message")')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.display('type-msg'), 'ValueError: test message')
})

test('runtime display', (t) => {
  const m = new Monty('raise ValueError("test message")')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.display('msg'), 'test message')
  t.is(error.display('type-msg'), 'ValueError: test message')
  const traceback = error.display('traceback')
  t.true(traceback.includes('Traceback (most recent call last):'))
  t.true(
    traceback.includes("raise ValueError('test message')") || traceback.includes('raise ValueError("test message")'),
  )
  t.true(traceback.includes('ValueError: test message'))
})

test('str returns type msg', (t) => {
  const m = new Monty('raise ValueError("test message")')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'ValueError: test message')
})

test('syntax error display', (t) => {
  const error = t.throws(() => new Monty('def'), { instanceOf: MontySyntaxError })
  t.true(error.display().includes('Expected an identifier'))
  t.true(error.display('type-msg').includes('SyntaxError'))
})

// =============================================================================
// Traceback tests
// =============================================================================

test('traceback frames', (t) => {
  const code = `def inner():
    raise ValueError('error')

def outer():
    inner()

outer()
`
  const m = new Monty(code)
  const error = t.throws(() => m.run(), isRuntimeError)
  const display = error.display('traceback')

  t.true(display.includes('Traceback (most recent call last):'))
  t.true(display.includes('outer()'))
  t.true(display.includes('inner()'))
  t.true(display.includes('ValueError: error'))
})

// =============================================================================
// MontyError base class tests
// =============================================================================

test('MontyError extends Error', (t) => {
  const err = new MontyError('ValueError', 'test message')
  t.true(err instanceof Error)
  t.true(err instanceof MontyError)
  t.is(err.name, 'MontyError')
})

test('MontyError constructor and properties', (t) => {
  const err = new MontyError('ValueError', 'test message')
  t.deepEqual(err.exception, { typeName: 'ValueError', message: 'test message' })
  t.is(err.message, 'ValueError: test message')
})

test('MontyError display()', (t) => {
  const err = new MontyError('ValueError', 'test message')
  t.is(err.display('msg'), 'test message')
  t.is(err.display('type-msg'), 'ValueError: test message')
})

test('MontyError with empty message', (t) => {
  const err = new MontyError('TypeError', '')
  t.is(err.display('type-msg'), 'TypeError')
})

// =============================================================================
// MontySyntaxError class tests
// =============================================================================

test('MontySyntaxError extends MontyError and Error', (t) => {
  const err = new MontySyntaxError('invalid syntax')
  t.true(err instanceof Error)
  t.true(err instanceof MontyError)
  t.true(err instanceof MontySyntaxError)
  t.is(err.name, 'MontySyntaxError')
})

test('MontySyntaxError constructor and properties', (t) => {
  const err = new MontySyntaxError('invalid syntax')
  t.deepEqual(err.exception, { typeName: 'SyntaxError', message: 'invalid syntax' })
  t.is(err.message, 'SyntaxError: invalid syntax')
})

test('MontySyntaxError display()', (t) => {
  const err = new MontySyntaxError('unexpected token')
  t.is(err.display(), 'unexpected token')
  t.is(err.display('msg'), 'unexpected token')
  t.is(err.display('type-msg'), 'SyntaxError: unexpected token')
})

// =============================================================================
// MontyRuntimeError class tests
// =============================================================================

test('MontyRuntimeError display()', (t) => {
  const m = new Monty('1 / 0')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)

  t.is(error.message, 'ZeroDivisionError: division by zero')

  const traceback = error.display('traceback')
  t.is(error.display(), traceback)
  t.true(traceback.includes('Traceback (most recent call last):'))

  t.is(error.display('type-msg'), 'ZeroDivisionError: division by zero')
  t.is(error.display('msg'), 'division by zero')
})

test('MontyRuntimeError can be caught with instanceof', (t) => {
  const m = new Monty('1 / 0')
  try {
    m.run()
    t.fail('Should have thrown')
  } catch (e) {
    t.true(e instanceof MontyRuntimeError)
    t.true(e instanceof MontyError)
    t.true(e instanceof Error)
  }
})

// =============================================================================
// MontyTypingError class tests
// =============================================================================

test('MontyTypingError extends MontyError and Error', (t) => {
  const err = new MontyTypingError('type mismatch')
  t.true(err instanceof Error)
  t.true(err instanceof MontyError)
  t.true(err instanceof MontyTypingError)
  t.is(err.name, 'MontyTypingError')
})

test('MontyTypingError is thrown on type check failure', (t) => {
  const code = `
x: int = "not an int"
`
  const error = t.throws(() => new Monty(code, { typeCheck: true }), { instanceOf: MontyTypingError })
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)
})

// =============================================================================
// Error catching hierarchy tests
// =============================================================================

test('MontyError catches all Monty exceptions', (t) => {
  // Syntax error
  try {
    new Monty('def')
  } catch (e) {
    t.true(e instanceof MontyError)
  }

  // Runtime error
  try {
    new Monty('1 / 0').run()
  } catch (e) {
    t.true(e instanceof MontyError)
  }

  // Type error
  try {
    new Monty('x: int = "str"', { typeCheck: true })
  } catch (e) {
    t.true(e instanceof MontyError)
  }
})

test('can distinguish error types with instanceof', (t) => {
  // Test syntax error
  try {
    new Monty('def')
  } catch (e) {
    t.true(e instanceof MontySyntaxError)
    t.false(e instanceof MontyRuntimeError)
    t.false(e instanceof MontyTypingError)
  }

  // Test runtime error
  try {
    new Monty('1 / 0').run()
  } catch (e) {
    t.true(e instanceof MontyRuntimeError)
    t.false(e instanceof MontySyntaxError)
    t.false(e instanceof MontyTypingError)
  }

  // Test type error
  try {
    new Monty('x: int = "str"', { typeCheck: true })
  } catch (e) {
    t.true(e instanceof MontyTypingError)
    t.false(e instanceof MontySyntaxError)
    t.false(e instanceof MontyRuntimeError)
  }
})

// =============================================================================
// Exception info accessors tests
// =============================================================================

test('exception getter returns correct info for runtime error', (t) => {
  const m = new Monty('raise ValueError("test")')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.exception.typeName, 'ValueError')
  t.is(error.exception.message, 'test')
})

test('exception getter returns correct info for syntax error', (t) => {
  const error = t.throws(() => new Monty('def'), { instanceOf: MontySyntaxError })
  t.is(error.exception.typeName, 'SyntaxError')
})

// =============================================================================
// Polymorphic display() tests
// =============================================================================

test('display() works polymorphically on MontyTypingError', (t) => {
  try {
    new Monty('x: int = "str"', { typeCheck: true })
    t.fail('Should have thrown')
  } catch (e) {
    t.true(e instanceof MontyError)
    const msg = (e as MontyError).display('msg')
    t.true(msg.length > 0)
    const typeMsg = (e as MontyError).display('type-msg')
    t.true(typeMsg.startsWith('TypeError:'))
  }
})
