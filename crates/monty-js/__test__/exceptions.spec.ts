import test from 'ava'

import { MontyError, MontySyntaxError, MontyRuntimeError, MontyTypingError } from '../ts/index.js'
import { setupPool } from './helpers.js'

const { run } = setupPool(test)

const isRuntimeError = { instanceOf: MontyRuntimeError }
const isSyntaxError = { instanceOf: MontySyntaxError }
const isTypingError = { instanceOf: MontyTypingError }

// =============================================================================
// MontyRuntimeError tests
// =============================================================================

test('zero division error', async (t) => {
  const error = await t.throwsAsync(() => run('1 / 0'), isRuntimeError)
  t.is(error.message, 'ZeroDivisionError: division by zero')
})

test('value error', async (t) => {
  const error = await t.throwsAsync(() => run('raise ValueError("bad value")'), isRuntimeError)
  t.is(error.message, 'ValueError: bad value')
})

test('type error', async (t) => {
  const error = await t.throwsAsync(() => run("'string' + 1"), isRuntimeError)
  t.is(error.message, 'TypeError: can only concatenate str (not "int") to str')
})

test('unicode encode error', async (t) => {
  const error = await t.throwsAsync(() => run('"café".encode("ascii")'), isRuntimeError)
  t.is(error.exception.typeName, 'UnicodeEncodeError')
  t.is(
    error.message,
    "UnicodeEncodeError: 'ascii' codec can't encode character '\\xe9' in position 3: ordinal not in range(128)",
  )
})

test('unicode decode error', async (t) => {
  const error = await t.throwsAsync(() => run('b"\\xe9".decode("ascii")'), isRuntimeError)
  t.is(error.exception.typeName, 'UnicodeDecodeError')
  t.is(
    error.message,
    "UnicodeDecodeError: 'ascii' codec can't decode byte 0xe9 in position 0: ordinal not in range(128)",
  )
})

test('index error', async (t) => {
  const error = await t.throwsAsync(() => run('[1, 2, 3][10]'), isRuntimeError)
  t.is(error.message, 'IndexError: list index out of range')
})

test('key error', async (t) => {
  const error = await t.throwsAsync(() => run('{"a": 1}["b"]'), isRuntimeError)
  t.is(error.message, 'KeyError: b')
})

test('attribute error', async (t) => {
  const error = await t.throwsAsync(() => run('raise AttributeError("no such attr")'), isRuntimeError)
  t.is(error.message, 'AttributeError: no such attr')
})

test('name error', async (t) => {
  const error = await t.throwsAsync(() => run('undefined_variable'), isRuntimeError)
  t.is(error.message, "NameError: name 'undefined_variable' is not defined")
})

test('assertion error', async (t) => {
  const error = await t.throwsAsync(() => run('assert False'), isRuntimeError)
  t.is(error.message, 'AssertionError')
})

test('assertion error with message', async (t) => {
  const error = await t.throwsAsync(() => run('assert False, "custom message"'), isRuntimeError)
  t.is(error.message, 'AssertionError: custom message')
})

test('runtime error', async (t) => {
  const error = await t.throwsAsync(() => run('raise RuntimeError("runtime error")'), isRuntimeError)
  t.is(error.message, 'RuntimeError: runtime error')
})

test('not implemented error', async (t) => {
  const error = await t.throwsAsync(() => run('raise NotImplementedError("not implemented")'), isRuntimeError)
  t.is(error.message, 'NotImplementedError: not implemented')
})

// =============================================================================
// OS call errors (no `os` callback given to the feed)
// =============================================================================

test('os.environ without os callback raises RuntimeError', async (t) => {
  const error = await t.throwsAsync(() => run('import os\nx = os.environ'), isRuntimeError)
  t.is(error.exception.typeName, 'RuntimeError')
  t.is(error.exception.message, "'os.environ' is not supported in this environment")
})

test('os.getenv without os callback raises RuntimeError', async (t) => {
  const error = await t.throwsAsync(() => run("import os\nx = os.getenv('HOME')"), isRuntimeError)
  t.is(error.exception.typeName, 'RuntimeError')
  t.is(error.exception.message, "'os.getenv' is not supported in this environment")
})

// =============================================================================
// MontySyntaxError tests
// =============================================================================

test('syntax error on run', async (t) => {
  const error = await t.throwsAsync(() => run('def'), isSyntaxError)
  t.is(error.message, 'SyntaxError: Expected an identifier')
})

test('syntax error unclosed paren', async (t) => {
  const error = await t.throwsAsync(() => run('print(1'), isSyntaxError)
  t.is(error.message, 'SyntaxError: unexpected EOF while parsing')
})

test('syntax error invalid syntax', async (t) => {
  const error = await t.throwsAsync(() => run('x = = 1'), isSyntaxError)
  t.is(error.message, 'SyntaxError: Expected an expression')
})

// =============================================================================
// Catching with base class tests
// =============================================================================

test('catch with base class', async (t) => {
  const error = await t.throwsAsync(() => run('1 / 0'))
  t.true(error instanceof MontyError)
})

test('catch syntax error with base class', async (t) => {
  const error = await t.throwsAsync(() => run('def'))
  t.true(error instanceof MontyError)
})

// =============================================================================
// Exception handling within Monty tests
// =============================================================================

test('raise caught exception', async (t) => {
  const code = `
try:
    1 / 0
except ZeroDivisionError as e:
    result = 'caught'
result
`
  t.is(await run(code), 'caught')
})

test('exception in function', async (t) => {
  const code = `
def fail():
    raise ValueError('from function')

fail()
`
  const error = await t.throwsAsync(() => run(code), isRuntimeError)
  t.is(error.message, 'ValueError: from function')
})

// =============================================================================
// Display and str methods tests
// =============================================================================

test('display traceback', async (t) => {
  const error = await t.throwsAsync(() => run('1 / 0'), isRuntimeError)
  t.is(
    error.display('traceback'),
    `Traceback (most recent call last):
  File "<python-input-0>", line 1, in <module>
    1 / 0
    ~~~~~
ZeroDivisionError: division by zero`,
  )
})

test('display type msg', async (t) => {
  const error = await t.throwsAsync(() => run('raise ValueError("test message")'), isRuntimeError)
  t.is(error.display('type-msg'), 'ValueError: test message')
})

test('runtime display', async (t) => {
  const error = await t.throwsAsync(() => run('raise ValueError("test message")'), isRuntimeError)
  t.is(error.display('msg'), 'test message')
  t.is(error.display('type-msg'), 'ValueError: test message')
  t.is(
    error.display('traceback'),
    `Traceback (most recent call last):
  File "<python-input-0>", line 1, in <module>
    raise ValueError("test message")
ValueError: test message`,
  )
})

test('str returns type msg', async (t) => {
  const error = await t.throwsAsync(() => run('raise ValueError("test message")'), isRuntimeError)
  t.is(error.message, 'ValueError: test message')
})

test('syntax error display', async (t) => {
  const error = await t.throwsAsync(() => run('def'), isSyntaxError)
  t.is(error.display(), 'Expected an identifier')
  t.is(error.display('type-msg'), 'SyntaxError: Expected an identifier')
})

// =============================================================================
// Traceback tests
// =============================================================================

test('traceback frames', async (t) => {
  const code = `def inner():
    raise ValueError('error')

def outer():
    inner()

outer()
`
  const error = await t.throwsAsync(() => run(code), isRuntimeError)
  t.is(
    error.display('traceback'),
    `Traceback (most recent call last):
  File "<python-input-0>", line 7, in <module>
    outer()
    ~~~~~~~
  File "<python-input-0>", line 5, in outer
    inner()
    ~~~~~~~
  File "<python-input-0>", line 2, in inner
    raise ValueError('error')
ValueError: error`,
  )
})

test('traceback() returns structured frames', async (t) => {
  const code = `def inner():
    raise ValueError('error')

def outer():
    inner()

outer()
`
  const error = await t.throwsAsync(() => run(code), isRuntimeError)
  t.deepEqual(error.traceback(), [
    {
      filename: '<python-input-0>',
      line: 7,
      column: 1,
      endLine: 7,
      endColumn: 8,
      functionName: '<module>',
      sourceLine: 'outer()',
    },
    {
      filename: '<python-input-0>',
      line: 5,
      column: 5,
      endLine: 5,
      endColumn: 12,
      functionName: 'outer',
      sourceLine: '    inner()',
    },
    {
      filename: '<python-input-0>',
      line: 2,
      column: 11,
      endLine: 2,
      endColumn: 30,
      functionName: 'inner',
      sourceLine: "    raise ValueError('error')",
    },
  ])
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

test('MontyRuntimeError display()', async (t) => {
  const error = await t.throwsAsync(() => run('1 / 0'), isRuntimeError)
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)

  t.is(error.message, 'ZeroDivisionError: division by zero')

  const traceback = error.display('traceback')
  t.is(error.display(), traceback)
  t.is(
    traceback,
    `Traceback (most recent call last):
  File "<python-input-0>", line 1, in <module>
    1 / 0
    ~~~~~
ZeroDivisionError: division by zero`,
  )

  t.is(error.display('type-msg'), 'ZeroDivisionError: division by zero')
  t.is(error.display('msg'), 'division by zero')
})

test('MontyRuntimeError can be caught with instanceof', async (t) => {
  const error = await t.throwsAsync(() => run('1 / 0'))
  t.true(error instanceof MontyRuntimeError)
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)
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

test('MontyTypingError is thrown on type check failure', async (t) => {
  const error = await t.throwsAsync(() => run('x: int = "not an int"', { typeCheck: true }), isTypingError)
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)
  t.is(
    error.message,
    'TypeError: error[invalid-assignment]: Object of type `Literal["not an int"]` is not assignable to `int`',
  )
  t.is(
    error.display(),
    'error[invalid-assignment]: Object of type `Literal["not an int"]` is not assignable to `int`\n' +
      ' --> main.py:1:4\n' +
      '  |\n' +
      '1 | x: int = "not an int"\n' +
      '  |    ---   ^^^^^^^^^^^^ Incompatible value of type `Literal["not an int"]`\n' +
      '  |    |\n' +
      '  |    Declared type\n' +
      '  |\n\n',
  )
})

// =============================================================================
// Error catching hierarchy tests
// =============================================================================

test('MontyError catches all Monty exceptions', async (t) => {
  // Syntax error
  t.true((await t.throwsAsync(() => run('def'))) instanceof MontyError)
  // Runtime error
  t.true((await t.throwsAsync(() => run('1 / 0'))) instanceof MontyError)
  // Typing error
  t.true((await t.throwsAsync(() => run('x: int = "str"', { typeCheck: true }))) instanceof MontyError)
})

test('can distinguish error types with instanceof', async (t) => {
  // Test syntax error
  const syntaxError = await t.throwsAsync(() => run('def'))
  t.true(syntaxError instanceof MontySyntaxError)
  t.false(syntaxError instanceof MontyRuntimeError)
  t.false(syntaxError instanceof MontyTypingError)

  // Test runtime error
  const runtimeError = await t.throwsAsync(() => run('1 / 0'))
  t.true(runtimeError instanceof MontyRuntimeError)
  t.false(runtimeError instanceof MontySyntaxError)
  t.false(runtimeError instanceof MontyTypingError)

  // Test typing error
  const typingError = await t.throwsAsync(() => run('x: int = "str"', { typeCheck: true }))
  t.true(typingError instanceof MontyTypingError)
  t.false(typingError instanceof MontySyntaxError)
  t.false(typingError instanceof MontyRuntimeError)
})

// =============================================================================
// Exception info accessors tests
// =============================================================================

test('exception getter returns correct info for runtime error', async (t) => {
  const error = await t.throwsAsync(() => run('raise ValueError("test")'), isRuntimeError)
  t.is(error.exception.typeName, 'ValueError')
  t.is(error.exception.message, 'test')
})

test('exception getter returns correct info for syntax error', async (t) => {
  const error = await t.throwsAsync(() => run('def'), isSyntaxError)
  t.is(error.exception.typeName, 'SyntaxError')
})

// =============================================================================
// Polymorphic display() tests
// =============================================================================

test('display() works polymorphically on MontyTypingError', async (t) => {
  const error = await t.throwsAsync(() => run('x: int = "str"', { typeCheck: true }))
  t.true(error instanceof MontyError)
  // MontyTypingError.display() always returns the rendered diagnostics,
  // whatever format is requested via the base-class signature.
  const msg = (error as MontyError).display('msg')
  t.true(msg.startsWith('error[invalid-assignment]:'))
  t.is(
    error.message,
    'TypeError: error[invalid-assignment]: Object of type `Literal["str"]` is not assignable to `int`',
  )
})
