import test from 'ava'

import type { ErrorConstructor } from 'ava'

import { Monty, MontyError, MontySyntaxError, MontyRuntimeError, MontyTypingError } from '../wrapper'

// =============================================================================
// MontyError tests
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
// MontySyntaxError tests
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

test('MontySyntaxError is thrown on syntax error', (t) => {
  const error = t.throws(() => new Monty('def'), { instanceOf: MontySyntaxError })
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)
})

test('MontySyntaxError can be caught with instanceof', (t) => {
  try {
    new Monty('def')
    t.fail('Should have thrown')
  } catch (e) {
    t.true(e instanceof MontySyntaxError)
    t.true(e instanceof MontyError)
    t.true(e instanceof Error)
  }
})

// =============================================================================
// MontyRuntimeError tests
// =============================================================================

// Helper for asserting MontyRuntimeError, private constructor requires the awkward cast via any
// but it works fine at runtime
export const isRuntimeError = { instanceOf: MontyRuntimeError as any as ErrorConstructor<MontyRuntimeError> }

test('MontyRuntimeError display()', (t) => {
  const m = new Monty('1 / 0')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)

  t.is(error.message, 'ZeroDivisionError: division by zero')

  const traceback = error.display('traceback')

  t.is(error.display(), traceback)
  t.is(
    traceback,
    `Traceback (most recent call last):
  File "main.py", line 1, in <module>
    1 / 0
    ~~~~~
ZeroDivisionError: division by zero`,
  )

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

test('ValueError message is preserved', (t) => {
  const m = new Monty('raise ValueError("custom message")')
  const error = t.throws<MontyRuntimeError>(() => m.run(), isRuntimeError)
  t.is(error.message, 'ValueError: custom message')
})

test('TypeError message is preserved', (t) => {
  const m = new Monty("'hello' + 123")
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'TypeError: can only concatenate str (not "int") to str')
})

test('KeyError shows the key', (t) => {
  const m = new Monty('{"a": 1}["missing"]')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'KeyError: missing')
})

test('IndexError shows index out of range', (t) => {
  const m = new Monty('[1, 2, 3][100]')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'IndexError: list index out of range')
})

test('NameError shows undefined variable', (t) => {
  const m = new Monty('undefined_var')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, "NameError: name 'undefined_var' is not defined")
})

test('AssertionError from assert statement', (t) => {
  const m = new Monty('assert False, "assertion failed"')
  const error = t.throws(() => m.run(), isRuntimeError)
  t.is(error.message, 'AssertionError: assertion failed')
})

test('RecursionError on deep recursion', (t) => {
  const code = `
def recurse(n):
    return recurse(n + 1)
recurse(0)
`
  const m = new Monty(code)
  const error = t.throws(() => m.run({ limits: { maxRecursionDepth: 10 } }), isRuntimeError)
  t.is(error.message, 'RecursionError: maximum recursion depth exceeded')
})

// =============================================================================
// MontyTypingError tests
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

test('MontyTypingError from typeCheck method', (t) => {
  const code = `
def foo(x: int) -> str:
    return x  # type error: returning int instead of str
`
  const m = new Monty(code)
  const error = t.throws(() => m.typeCheck(), { instanceOf: MontyTypingError })
  t.true(error instanceof MontyError)
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
  const syntaxCode = 'def'
  const runtimeCode = '1 / 0'
  const typeCode = 'x: int = "str"'

  // Test syntax error
  try {
    new Monty(syntaxCode)
  } catch (e) {
    t.true(e instanceof MontySyntaxError)
    t.false(e instanceof MontyRuntimeError)
    t.false(e instanceof MontyTypingError)
  }

  // Test runtime error
  try {
    new Monty(runtimeCode).run()
  } catch (e) {
    t.true(e instanceof MontyRuntimeError)
    t.false(e instanceof MontySyntaxError)
    t.false(e instanceof MontyTypingError)
  }

  // Test type error
  try {
    new Monty(typeCode, { typeCheck: true })
  } catch (e) {
    t.true(e instanceof MontyTypingError)
    t.false(e instanceof MontySyntaxError)
    t.false(e instanceof MontyRuntimeError)
  }
})

// =============================================================================
// Exception info accessors
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
