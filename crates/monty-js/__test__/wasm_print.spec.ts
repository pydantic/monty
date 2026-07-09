import { test } from 'vitest'
import { t } from './assertions.js'
import { Monty, type ResourceLimits, MontySnapshot, MontyComplete } from '../ts/wasm.js'

// =============================================================================
// Print tests
// =============================================================================

function makePrintCollector() {
  const output: string[] = []

  const callback = (stream: string, text: string) => {
    t.is(stream, 'stdout')
    output.push(text)
  }

  return { callback, output }
}

test('basic', () => {
  const m = new Monty('print("hello")')
  const { output, callback } = makePrintCollector()
  m.run({ printCallback: callback })
  t.is(output.join(''), 'hello\n')
})

test('multiple', () => {
  const m = new Monty('print("hello")\nprint("world")')
  const { output, callback } = makePrintCollector()
  m.run({ printCallback: callback })
  t.is(output.join(''), 'hello\nworld\n')
})

test('with values', () => {
  const m = new Monty('print("The answer is", 42)')
  const { output, callback } = makePrintCollector()
  m.run({ printCallback: callback })
  t.is(output.join(''), 'The answer is 42\n')
})

test('with step', () => {
  const m = new Monty('print(1, 2, 3, sep="-")')
  const { output, callback } = makePrintCollector()
  m.run({ printCallback: callback })
  t.is(output.join(''), '1-2-3\n')
})

test('with end', () => {
  const m = new Monty('print("hello", end="!")')
  const { output, callback } = makePrintCollector()
  m.run({ printCallback: callback })
  t.is(output.join(''), 'hello!')
})

test('returns none', () => {
  const m = new Monty('result = print("hello")')
  const { callback } = makePrintCollector()
  const result = m.run({ printCallback: callback })
  t.is(result, null)
})

test('empty', () => {
  const m = new Monty('print()')
  const { output, callback } = makePrintCollector()
  m.run({ printCallback: callback })
  t.is(output.join(''), '\n')
})

test('with limits', () => {
  const m = new Monty('print("with limits")')
  const { output, callback } = makePrintCollector()
  const limits: ResourceLimits = {
    maxDurationSecs: 5.0,
  }
  m.run({ printCallback: callback, limits })
  t.is(output.join(''), 'with limits\n')
})

test('with inputs', () => {
  const m = new Monty('print("Input value is", x)', { inputs: ['x'] })
  const { output, callback } = makePrintCollector()
  m.run({ inputs: { x: 99 }, printCallback: callback })
  t.is(output.join(''), 'Input value is 99\n')
})

test('print in loop', () => {
  const code = `
for i in range(3):
	print("Count", i)
`
  const m = new Monty(code)
  const { output, callback } = makePrintCollector()
  m.run({ printCallback: callback })
  t.is(output.join(''), 'Count 0\nCount 1\nCount 2\n')
})

test('print mixed types', () => {
  const m = new Monty('print("Value:", 3.14, True, None, [1, 2, 3])')
  const { output, callback } = makePrintCollector()
  m.run({ printCallback: callback })
  t.is(output.join(''), 'Value: 3.14 True None [1, 2, 3]\n')
})

function makeErrorCallback(error: Error) {
  const output: string[] = []

  const callback = (stream: string, text: string) => {
    const _ignore = text
    t.is(stream, 'stdout')
    throw error
  }

  return { callback, output }
}

test('raises error', () => {
  const m = new Monty('print("This will error")')
  const error = new Error('Custom print error')
  const { callback } = makeErrorCallback(error)
  const thrown = t.throws(() => {
    m.run({ printCallback: callback })
  })
  // the error is slightly different with WASI, it doesn't include "Error: "
  t.regex(thrown?.message, /Exception: (:?Error: )?Custom print error/)
})

test('raises in function', () => {
  const code = `
def greet(name):
	print(f"Hello, {name}!")

greet("Alice")
`
  const m = new Monty(code)
  const error = new Error('Print error in function')
  const { callback } = makeErrorCallback(error)
  const thrown = t.throws(() => {
    m.run({ printCallback: callback })
  })
  // the error is slightly different with WASI, it doesn't include "Error: "
  t.regex(thrown?.message, /Exception: (:?Error: )?Print error in function/)
})

test('raises in nested function', () => {
  const code = `
def outer():
	def inner():
		print("Inside inner function")
	inner()

outer()
`
  const m = new Monty(code)
  const error = new Error('Print error in nested function')
  const { callback } = makeErrorCallback(error)
  const thrown = t.throws(() => {
    m.run({ printCallback: callback })
  })
  // the error is slightly different with WASI, it doesn't include "Error: "
  t.regex(thrown?.message, /Exception: (:?Error: )?Print error in nested function/)
})

test('raises in loop', () => {
  const code = `
for i in range(3):
	print(f"Count: {i}")
`
  const m = new Monty(code)
  const error = new Error('Print error in loop')
  const { callback } = makeErrorCallback(error)
  const thrown = t.throws(() => {
    m.run({ printCallback: callback })
  })
  // the error is slightly different with WASI, it doesn't include "Error: "
  t.regex(thrown?.message, /Exception: (:?Error: )?Print error in loop/)
})

test('with snapshot', () => {
  const m = new Monty('print("snapshot")')
  const { output, callback } = makePrintCollector()
  const result = m.start({
    printCallback: callback,
  })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, null)
  t.is(output.join(''), 'snapshot\n')
})

test('with snapshot resume', () => {
  const code = `
print("hello")
print(func())
`
  const m = new Monty(code)
  const { output, callback } = makePrintCollector()
  const progress = m.start({
    printCallback: callback,
  })
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  const result = snapshot.resume({
    returnValue: 'world',
  })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, null)
  t.is(output.join(''), 'hello\nworld\n')
})

test('with snapshot dump load', () => {
  const m = new Monty('print(func())')
  const { output, callback } = makePrintCollector()

  const progress = m.start({
    printCallback: callback,
  })
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  const data = snapshot.dump()

  const progress2 = MontySnapshot.load(data, {
    printCallback: callback,
  })
  const result = progress2.resume({
    returnValue: 42,
  })
  t.true(result instanceof MontyComplete)
  t.is((result as MontyComplete).output, null)
  t.is(output.join(''), '42\n')
})
