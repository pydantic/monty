import { test } from 'vitest'
import { t } from './assertions.js'

import { setupPool } from './helpers.js'

const { run } = setupPool()

// =============================================================================
// Print tests
// =============================================================================

// Collects printCallback invocations. Output is line-buffered: each callback
// call receives one whole line including its trailing '\n' (or the unflushed
// tail of the stream at the end of the turn).
function makePrintCollector() {
  const output: string[] = []

  const callback = (stream: 'stdout' | 'stderr', text: string) => {
    t.is(stream, 'stdout')
    output.push(text)
  }

  return { callback, output }
}

test('basic', async () => {
  const { output, callback } = makePrintCollector()
  await run('print("hello")', { printCallback: callback })
  t.deepEqual(output, ['hello\n'])
})

test('multiple', async () => {
  const { output, callback } = makePrintCollector()
  await run('print("hello")\nprint("world")', { printCallback: callback })
  t.deepEqual(output, ['hello\n', 'world\n'])
})

test('with values', async () => {
  const { output, callback } = makePrintCollector()
  await run('print("The answer is", 42)', { printCallback: callback })
  t.deepEqual(output, ['The answer is 42\n'])
})

test('with step', async () => {
  const { output, callback } = makePrintCollector()
  await run('print(1, 2, 3, sep="-")', { printCallback: callback })
  t.deepEqual(output, ['1-2-3\n'])
})

test('with end', async () => {
  const { output, callback } = makePrintCollector()
  await run('print("hello", end="!")', { printCallback: callback })
  // No trailing newline: the partial line is flushed once, at the end of the turn.
  t.deepEqual(output, ['hello!'])
})

test('partial lines are buffered until a newline', async () => {
  const { output, callback } = makePrintCollector()
  await run('print("a", end="")\nprint("b")', { printCallback: callback })
  t.deepEqual(output, ['ab\n'])
})

test('returns none', async () => {
  const { callback } = makePrintCollector()
  const result = await run('result = print("hello")', { printCallback: callback })
  t.is(result, null)
})

test('empty', async () => {
  const { output, callback } = makePrintCollector()
  await run('print()', { printCallback: callback })
  t.deepEqual(output, ['\n'])
})

test('with limits', async () => {
  const { output, callback } = makePrintCollector()
  await run('print("with limits")', { printCallback: callback, limits: { maxDurationSecs: 5.0 } })
  t.deepEqual(output, ['with limits\n'])
})

test('with inputs', async () => {
  const { output, callback } = makePrintCollector()
  await run('print("Input value is", x)', { inputs: { x: 99 }, printCallback: callback })
  t.deepEqual(output, ['Input value is 99\n'])
})

test('print in loop', async () => {
  const code = `
for i in range(3):
	print("Count", i)
`
  const { output, callback } = makePrintCollector()
  await run(code, { printCallback: callback })
  t.deepEqual(output, ['Count 0\n', 'Count 1\n', 'Count 2\n'])
})

test('print mixed types', async () => {
  const { output, callback } = makePrintCollector()
  await run('print("Value:", 3.14, True, None, [1, 2, 3])', { printCallback: callback })
  t.deepEqual(output, ['Value: 3.14 True None [1, 2, 3]\n'])
})

// =============================================================================
// Throwing print callbacks: the feed rejects with the callback's error
// =============================================================================

function makeErrorCallback(error: Error) {
  const callback = (stream: 'stdout' | 'stderr', _text: string) => {
    t.is(stream, 'stdout')
    throw error
  }

  return { callback }
}

test('raises error', async () => {
  const error = new Error('Custom print error')
  const { callback } = makeErrorCallback(error)
  const thrown = await t.throwsAsync(() => run('print("This will error")', { printCallback: callback }))
  t.is(thrown, error)
  t.is(thrown.message, 'Custom print error')
})

test('raises in function', async () => {
  const code = `
def greet(name):
	print(f"Hello, {name}!")

greet("Alice")
`
  const error = new Error('Print error in function')
  const { callback } = makeErrorCallback(error)
  const thrown = await t.throwsAsync(() => run(code, { printCallback: callback }))
  t.is(thrown, error)
})

test('raises in nested function', async () => {
  const code = `
def outer():
	def inner():
		print("Inside inner function")
	inner()

outer()
`
  const error = new Error('Print error in nested function')
  const { callback } = makeErrorCallback(error)
  const thrown = await t.throwsAsync(() => run(code, { printCallback: callback }))
  t.is(thrown, error)
})

test('raises in loop', async () => {
  const code = `
for i in range(3):
	print(f"Count: {i}")
`
  const error = new Error('Print error in loop')
  const { callback } = makeErrorCallback(error)
  const thrown = await t.throwsAsync(() => run(code, { printCallback: callback }))
  t.is(thrown, error)
})

// =============================================================================
// Print interleaved with external function calls (was the snapshot/resume test)
// =============================================================================

test('print with external function result', async () => {
  const code = `
print("hello")
print(func())
`
  const { output, callback } = makePrintCollector()
  const result = await run(code, { printCallback: callback, externalLookup: { func: () => 'world' } })
  t.is(result, null)
  t.deepEqual(output, ['hello\n', 'world\n'])
})
