import test from 'ava'

import { MontySyntaxError } from '../ts/index.js'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool(test)

// =============================================================================
// Simple expression tests
// =============================================================================

test('simple expression', async (t) => {
  t.is(await run('1 + 2'), 3)
})

test('arithmetic', async (t) => {
  t.is(await run('10 * 5 - 3'), 47)
})

test('string concatenation', async (t) => {
  t.is(await run('"hello" + " " + "world"'), 'hello world')
})

test('syntax error', async (t) => {
  const error = await t.throwsAsync(() => run('def'), { instanceOf: MontySyntaxError })
  t.true(error.message.includes('SyntaxError'))
})

// =============================================================================
// Multiline code tests
// =============================================================================

test('multiline code', async (t) => {
  const code = `
x = 1
y = 2
x + y
`
  t.is(await run(code), 3)
})

test('function definition and call', async (t) => {
  const code = `
def add(a, b):
    return a + b

add(3, 4)
`
  t.is(await run(code), 7)
})

// =============================================================================
// Session behaviour
// =============================================================================

test('session state persists across feeds', async (t) => {
  const session = await pool().checkout()
  try {
    t.is(await session.feedRun('x = 5'), null)
    t.is(await session.feedRun('x * 2'), 10)
  } finally {
    await session.close()
  }
})

test('sessions are isolated from each other', async (t) => {
  const a = await pool().checkout()
  const b = await pool().checkout()
  try {
    await a.feedRun('secret = 42')
    const error = await t.throwsAsync(() => b.feedRun('secret'))
    t.is(error.message, "NameError: name 'secret' is not defined")
  } finally {
    await a.close()
    await b.close()
  }
})

test('await using closes the session', async (t) => {
  let result: unknown
  {
    await using session = await pool().checkout()
    result = await session.feedRun('21 * 2')
  }
  t.is(result, 42)
})
