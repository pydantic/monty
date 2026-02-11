import type { ExecutionContext } from 'ava'
import test from 'ava'
import { Monty, type ResourceLimits, MontySnapshot, MontyComplete } from '../wrapper'

// =============================================================================
// Print tests
// =============================================================================

function makePrintCollector(t: ExecutionContext) {
  const output: string[] = []

  const callback = (stream: string, text: string) => {
    t.assert(stream === 'stdout')
    output.push(text)
  }

  return { callback, output }
}

test('basic', (t) => {
  const m = new Monty('print("hello")')
  const { output, callback } = makePrintCollector(t)
  m.run({ printCallback: callback })
  t.true(output.join('') === 'hello\n')
})

test('multiple', (t) => {
  const m = new Monty('print("hello")\nprint("world")')
  const { output, callback } = makePrintCollector(t)
  m.run({ printCallback: callback })
  t.true(output.join('') === 'hello\nworld\n')
})

test('with values', (t) => {
  const m = new Monty('print("The answer is", 42)')
  const { output, callback } = makePrintCollector(t)
  m.run({ printCallback: callback })
  t.true(output.join('') === 'The answer is 42\n')
})

test('with step', (t) => {
  const m = new Monty('print(1, 2, 3, sep="-")')
  const { output, callback } = makePrintCollector(t)
  m.run({ printCallback: callback })
  t.true(output.join('') === '1-2-3\n')
})

test('with end', (t) => {
  const m = new Monty('print("hello", end="!")')
  const { output, callback } = makePrintCollector(t)
  m.run({ printCallback: callback })
  t.true(output.join('') === 'hello!')
})

test('returns none', (t) => {
  const m = new Monty('result = print("hello")')
  const { callback } = makePrintCollector(t)
  const result = m.run({ printCallback: callback })
  t.assert(result === null)
})

test('empty', (t) => {
  const m = new Monty('print()')
  const { output, callback } = makePrintCollector(t)
  m.run({ printCallback: callback })
  t.true(output.join('') === '\n')
})

test('with limits', (t) => {
  const m = new Monty('print("with limits")')
  const { output, callback } = makePrintCollector(t)
  const limits: ResourceLimits = {
    maxDurationSecs: 5.0,
  }
  m.run({ printCallback: callback, limits })
  t.true(output.join('') === 'with limits\n')
})

test('with inputs', (t) => {
  const m = new Monty('print("Input value is", x)', { inputs: ['x'] })
  const { output, callback } = makePrintCollector(t)
  m.run({ inputs: { x: 99 }, printCallback: callback })
  t.true(output.join('') === 'Input value is 99\n')
})

test('print in loop', (t) => {
  const code = `
for i in range(3):
	print("Count", i)
`
  const m = new Monty(code)
  const { output, callback } = makePrintCollector(t)
  m.run({ printCallback: callback })
  t.true(output.join('') === 'Count 0\nCount 1\nCount 2\n')
})

test('print mixed types', (t) => {
  const m = new Monty('print("Value:", 3.14, True, None, [1, 2, 3])')
  const { output, callback } = makePrintCollector(t)
  m.run({ printCallback: callback })
  t.true(output.join('') === 'Value: 3.14 True None [1, 2, 3]\n')
})

function makeErrorCallback(error: Error, t: ExecutionContext) {
  const output: string[] = []

  const callback = (stream: string, text: string) => {
    const _ignore = text
    t.assert(stream === 'stdout')
    throw error
  }

  return { callback, output }
}

test('raises error', (t) => {
  const m = new Monty('print("This will error")')
  const error = new Error('Custom print error')
  const { callback } = makeErrorCallback(error, t)
  const thrown = t.throws(() => {
    m.run({ printCallback: callback })
  })
  t.assert(thrown?.message === 'Exception: Error: Custom print error')
})

test('raises in function', (t) => {
  const code = `
def greet(name):
	print(f"Hello, {name}!")

greet("Alice")
`
  const m = new Monty(code)
  const error = new Error('Print error in function')
  const { callback } = makeErrorCallback(error, t)
  const thrown = t.throws(() => {
    m.run({ printCallback: callback })
  })
  t.assert(thrown?.message === 'Exception: Error: Print error in function')
})

test('raises in nested function', (t) => {
  const code = `
def outer():
	def inner():
		print("Inside inner function")
	inner()

outer()
`
  const m = new Monty(code)
  const error = new Error('Print error in nested function')
  const { callback } = makeErrorCallback(error, t)
  const thrown = t.throws(() => {
    m.run({ printCallback: callback })
  })
  t.assert(thrown?.message === 'Exception: Error: Print error in nested function')
})

test('raises in loop', (t) => {
  const code = `
for i in range(3):
	print(f"Count: {i}")
`
  const m = new Monty(code)
  const error = new Error('Print error in loop')
  const { callback } = makeErrorCallback(error, t)
  const thrown = t.throws(() => {
    m.run({ printCallback: callback })
  })
  t.assert(thrown?.message === 'Exception: Error: Print error in loop')
})

test('with snapshot', (t) => {
  const m = new Monty('print("snapshot")')
  const { output, callback } = makePrintCollector(t)
  const result = m.start({
    printCallback: callback,
  })
  t.true(result instanceof MontyComplete)
  t.true((result as MontyComplete).output === null)
  t.true(output.join('') === 'snapshot\n')
})

test('with snapshot resume', (t) => {
  const code = `
print("hello")
print(func())
`
  const m = new Monty(code, { externalFunctions: ['func'] })
  const { output, callback } = makePrintCollector(t)
  const progress = m.start({
    printCallback: callback,
  })
  t.true(progress instanceof MontySnapshot)
  const snapshot = progress as MontySnapshot
  const result = snapshot.resume({
    returnValue: 'world',
  })
  t.true(result instanceof MontyComplete)
  t.true((result as MontyComplete).output === null)
  t.true(output.join('') === 'hello\nworld\n')
})

test('with snapshot dump load', (t) => {
  const m = new Monty('print(func())', {
    externalFunctions: ['func'],
  })
  const { output, callback } = makePrintCollector(t)

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
  t.true((result as MontyComplete).output === null)
  t.true(output.join('') === '42\n')
})
