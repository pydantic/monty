import test from 'ava'

import { MontyError, MontyRuntimeError, MontyTypingError } from '../ts/index.js'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool(test)

/** The full diagnostics rendering for `"hello" + 1`, parameterized by script name. */
const unsupportedOperatorDiagnostics = (scriptName: string) =>
  [
    'error[unsupported-operator]: Unsupported `+` operation',
    ` --> ${scriptName}:1:1`,
    '  |',
    '1 | "hello" + 1',
    '  | -------^^^-',
    '  | |         |',
    '  | |         Has type `Literal[1]`',
    '  | Has type `Literal["hello"]`',
    '  |',
    '',
    '',
  ].join('\n')

// =============================================================================
// typeCheck session option
// =============================================================================

test('type check no errors', async (t) => {
  t.is(await run('x = 1', { typeCheck: true }), null)
})

test('type check with errors', async (t) => {
  const error = await t.throwsAsync(() => run('"hello" + 1', { typeCheck: true }), { instanceOf: MontyTypingError })
  t.is(error.message, 'TypeError: error[unsupported-operator]: Unsupported `+` operation')
  t.is(error.display(), unsupportedOperatorDiagnostics('main.py'))
})

test('type check function return type', async (t) => {
  const code = `
def foo() -> int:
    return "not an int"
`
  const error = await t.throwsAsync(() => run(code, { typeCheck: true }), { instanceOf: MontyTypingError })
  t.is(error.message, 'TypeError: error[invalid-return-type]: Return type does not match returned value')
})

test('type check undefined variable', async (t) => {
  const error = await t.throwsAsync(() => run('print(undefined_var)', { typeCheck: true }), {
    instanceOf: MontyTypingError,
  })
  t.is(error.message, 'TypeError: error[unresolved-reference]: Name `undefined_var` used when not defined')
})

test('type check valid function', async (t) => {
  const code = `
def add(a: int, b: int) -> int:
    return a + b

add(1, 2)
`
  t.is(await run(code, { typeCheck: true }), 3)
})

test('type check disabled by default', async (t) => {
  // Without typeCheck the snippet executes and fails at runtime instead.
  const error = await t.throwsAsync(() => run('"hello" + 1'), { instanceOf: MontyRuntimeError })
  t.is(error.message, 'TypeError: can only concatenate str (not "int") to str')
})

test('type check explicit false', async (t) => {
  await t.throwsAsync(() => run('"hello" + 1', { typeCheck: false }), { instanceOf: MontyRuntimeError })
})

test('default allows run with inputs', async (t) => {
  // Type checking would reject the unresolved name, but it is off by default.
  t.is(await run('x + 1', { inputs: { x: 5 } }), 6)
})

// =============================================================================
// Accumulated type-check context and stubs
// =============================================================================

test('earlier feeds join the type-check context', async (t) => {
  // In a fresh session x is undefined ...
  await t.throwsAsync(() => run('result = x + 1', { typeCheck: true }), { instanceOf: MontyTypingError })
  // ... but a snippet fed earlier in the same session defines it.
  const session = await pool().checkout({ typeCheck: true })
  try {
    await session.feedRun('x = 0')
    t.is(await session.feedRun('result = x + 1\nresult'), 1)
  } finally {
    await session.close()
  }
})

test('type check stubs with external function', async (t) => {
  // The stub satisfies the type checker; the external function provides the
  // runtime implementation.
  t.is(
    await run('result = fetch("https://example.com")\nresult', {
      typeCheck: true,
      typeCheckStubs: 'def fetch(url: str) -> str: ...',
      externalLookup: { fetch: () => 'response data' },
    }),
    'response data',
  )
})

test('type check stubs invalid', async (t) => {
  // The stub defines x as str, but the code uses it in an int annotation context.
  const error = await t.throwsAsync(
    () => run('result: int = x + 1', { typeCheck: true, typeCheckStubs: 'x = "hello"' }),
    { instanceOf: MontyTypingError },
  )
  t.is(error.message, 'TypeError: error[unsupported-operator]: Unsupported `+` operation')
})

test('failing snippet does not execute and session survives', async (t) => {
  const session = await pool().checkout({ typeCheck: true })
  try {
    await session.feedRun('x = 1')
    await t.throwsAsync(() => session.feedRun('x = 2\n"hello" + 1'), { instanceOf: MontyTypingError })
    // The rejected snippet did not run: x is unchanged.
    t.is(await session.feedRun('x'), 1)
  } finally {
    await session.close()
  }
})

test('skipTypeCheck skips checking without joining the context', async (t) => {
  const session = await pool().checkout({ typeCheck: true })
  try {
    // The skipped feed executes unchecked ...
    t.is(await session.feedRun('y = 41\ny', { skipTypeCheck: true }), 41)
    // ... but does not join the accumulated type-check context.
    const error = await t.throwsAsync(() => session.feedRun('y + 1'), { instanceOf: MontyTypingError })
    t.is(error.message, 'TypeError: error[unresolved-reference]: Name `y` used when not defined')
  } finally {
    await session.close()
  }
})

test('scriptName appears in diagnostics', async (t) => {
  const error = await t.throwsAsync(() => run('"hello" + 1', { typeCheck: true, scriptName: 'my_script.py' }), {
    instanceOf: MontyTypingError,
  })
  t.is(error.display(), unsupportedOperatorDiagnostics('my_script.py'))
})

// =============================================================================
// MontyTypingError
// =============================================================================

test('monty typing error is monty error subclass', async (t) => {
  const error = await t.throwsAsync(() => run('"hello" + 1', { typeCheck: true }), { instanceOf: MontyTypingError })
  t.true(error instanceof MontyError)
  t.true(error instanceof Error)
})

test('monty typing error message is first diagnostic line', async (t) => {
  const error = await t.throwsAsync(() => run('"hello" + 1', { typeCheck: true }), { instanceOf: MontyTypingError })
  t.is(error.message, `TypeError: ${error.display().split('\n', 1)[0]}`)
  t.deepEqual(error.exception, {
    typeName: 'TypeError',
    message: 'error[unsupported-operator]: Unsupported `+` operation',
  })
})
