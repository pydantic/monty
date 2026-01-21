import test from 'ava'

import { Monty, MontyError, MontySyntaxError, MontyRuntimeError, MontyTypingError, type Frame } from '../wrapper'

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

test('MontyRuntimeError extends MontyError and Error', (t) => {
  const frames: Frame[] = []
  const err = new MontyRuntimeError('ValueError', 'bad value', 'Traceback...', frames)
  t.true(err instanceof Error)
  t.true(err instanceof MontyError)
  t.true(err instanceof MontyRuntimeError)
  t.is(err.name, 'MontyRuntimeError')
})

test('MontyRuntimeError constructor and properties', (t) => {
  const frames: Frame[] = [
    {
      filename: 'test.py',
      line: 1,
      column: 1,
      endLine: 1,
      endColumn: 10,
      functionName: 'test_func',
      sourceLine: 'x = 1 / 0',
    },
  ]
  const err = new MontyRuntimeError('ZeroDivisionError', 'division by zero', 'Full Traceback', frames)

  t.deepEqual(err.exception, { typeName: 'ZeroDivisionError', message: 'division by zero' })
  t.is(err.message, 'Full Traceback')
  t.deepEqual(err.traceback(), frames)
})

test('MontyRuntimeError display()', (t) => {
  const err = new MontyRuntimeError('ValueError', 'bad value', 'Full Traceback Here', [])
  t.is(err.display(), 'Full Traceback Here')
  t.is(err.display('traceback'), 'Full Traceback Here')
  t.is(err.display('type-msg'), 'ValueError: bad value')
  t.is(err.display('msg'), 'bad value')
})

test('MontyRuntimeError is thrown on runtime error', (t) => {
  const m = new Monty('1 / 0')
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)
  t.true(error.message.includes('ZeroDivisionError'))
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

test('MontyRuntimeError traceback contains frames', (t) => {
  const code = `
def foo():
    raise ValueError("test error")

def bar():
    foo()

bar()
`
  const m = new Monty(code)
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  // The error message contains the traceback with function names
  t.true(error.message.includes('foo'))
  t.true(error.message.includes('bar'))
  t.true(error.message.includes('ValueError: test error'))
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
// Error message content tests
// =============================================================================

test('ValueError message is preserved', (t) => {
  const m = new Monty('raise ValueError("custom message")')
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('ValueError: custom message'))
})

test('TypeError message is preserved', (t) => {
  const m = new Monty("'hello' + 123")
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('TypeError'))
})

test('KeyError shows the key', (t) => {
  const m = new Monty('{"a": 1}["missing"]')
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('KeyError'))
  t.true(error.message.includes('missing'))
})

test('IndexError shows index out of range', (t) => {
  const m = new Monty('[1, 2, 3][100]')
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('IndexError'))
})

test('NameError shows undefined variable', (t) => {
  const m = new Monty('undefined_var')
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('NameError'))
  t.true(error.message.includes('undefined_var'))
})

test('AssertionError from assert statement', (t) => {
  const m = new Monty('assert False, "assertion failed"')
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('AssertionError'))
  t.true(error.message.includes('assertion failed'))
})

test('RecursionError on deep recursion', (t) => {
  const code = `
def recurse(n):
    return recurse(n + 1)
recurse(0)
`
  const m = new Monty(code)
  const error = t.throws(() => m.run({ limits: { maxRecursionDepth: 10 } }), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('RecursionError'))
})

// =============================================================================
// Traceback format tests
// =============================================================================

test('Traceback includes line numbers', (t) => {
  const code = `x = 1
y = 2
z = x / 0
`
  const m = new Monty(code, { scriptName: 'test.py' })
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('line 3'))
  t.true(error.message.includes('test.py'))
})

test('Traceback includes function names in call stack', (t) => {
  const code = `
def level3():
    raise RuntimeError("deep error")

def level2():
    level3()

def level1():
    level2()

level1()
`
  const m = new Monty(code)
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('level1'))
  t.true(error.message.includes('level2'))
  t.true(error.message.includes('level3'))
})

// =============================================================================
// Exception info accessors
// =============================================================================

test('exception getter returns correct info for runtime error', (t) => {
  const m = new Monty('raise ValueError("test")')
  const error = t.throws(() => m.run(), { instanceOf: MontyRuntimeError })
  t.is(error.exception.typeName, 'ValueError')
  t.is(error.exception.message, 'test')
})

test('exception getter returns correct info for syntax error', (t) => {
  const error = t.throws(() => new Monty('def'), { instanceOf: MontySyntaxError })
  t.is(error.exception.typeName, 'SyntaxError')
})
