import { test } from 'vitest'
import { t } from './assertions.js'

import { CollectString, CollectStreams, MontyRuntimeError } from '@pydantic/monty'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

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

// =============================================================================
// Host collectors (CollectString / CollectStreams) — Python pool parity
// =============================================================================

test('CollectString accumulates', async () => {
  const collector = new CollectString()
  const result = await run('print("a"); print("b", 1); 123', { printCallback: collector })
  t.is(result, 123)
  t.is(collector.output, 'a\nb 1\n')
})

test('CollectStreams accumulates with labels', async () => {
  const collector = new CollectStreams()
  const result = await run('print("a"); print("b", 1); 123', { printCallback: collector })
  t.is(result, 123)
  t.deepEqual(collector.output, [
    { stream: 'stdout', text: 'a\n' },
    { stream: 'stdout', text: 'b 1\n' },
  ])
})

test('CollectStreams preserves stderr stream label', () => {
  // Unit-level: pool may not surface sys.stderr the same way; lock label routing here.
  const collector = new CollectStreams()
  collector.write('stderr', 'err\n')
  t.deepEqual(collector.output, [{ stream: 'stderr', text: 'err\n' }])
})

test('CollectString maxBytes first write fails', async () => {
  const collector = new CollectString(100)
  const thrown = await t.throwsAsync<MontyRuntimeError>(() => run("print('x' * 200)", { printCallback: collector }))
  t.true(thrown instanceof MontyRuntimeError)
  t.is(thrown.exception.typeName, 'MemoryError')
  t.is(thrown.exception.message, 'memory limit exceeded: 201 bytes > 100 bytes')
  t.is(collector.output, '')
})

test('CollectStreams maxBytes first write fails with overhead', async () => {
  const collector = new CollectStreams(100)
  const thrown = await t.throwsAsync<MontyRuntimeError>(() => run("print('x' * 200)", { printCallback: collector }))
  t.true(thrown instanceof MontyRuntimeError)
  t.is(thrown.exception.typeName, 'MemoryError')
  // 201 payload bytes + 64 entry overhead
  t.is(thrown.exception.message, 'memory limit exceeded: 265 bytes > 100 bytes')
  t.deepEqual(collector.output, [])
})

test('CollectString partial success keeps prior buffer', async () => {
  // First print('a') → 'a\n' = 2 bytes; second print('x'*20) → 21 bytes; total 23 > 10.
  const collector = new CollectString(10)
  const thrown = await t.throwsAsync<MontyRuntimeError>(() =>
    run("print('a'); print('x' * 20)", { printCallback: collector }),
  )
  t.true(thrown instanceof MontyRuntimeError)
  t.is(thrown.exception.typeName, 'MemoryError')
  t.is(thrown.exception.message, 'memory limit exceeded: 23 bytes > 10 bytes')
  t.is(collector.output, 'a\n')
})

test('CollectStreams partial success keeps prior entries', async () => {
  // First entry: 2 + 64 = 66; second: 21 + 64 = 85; total 151 > 100.
  const collector = new CollectStreams(100)
  const thrown = await t.throwsAsync<MontyRuntimeError>(() =>
    run("print('a'); print('x' * 20)", { printCallback: collector }),
  )
  t.true(thrown instanceof MontyRuntimeError)
  t.is(thrown.exception.typeName, 'MemoryError')
  t.is(thrown.exception.message, 'memory limit exceeded: 151 bytes > 100 bytes')
  t.deepEqual(collector.output, [{ stream: 'stdout', text: 'a\n' }])
})

test('CollectString charges UTF-8 multi-byte characters', () => {
  // 'é' is 1 UTF-16 code unit but 2 UTF-8 bytes — .length would under-count.
  const collector = new CollectString(1)
  const thrown = t.throws<MontyRuntimeError>(() => collector.write('stdout', 'é'))
  t.true(thrown instanceof MontyRuntimeError)
  t.is(thrown.exception.typeName, 'MemoryError')
  t.is(thrown.exception.message, 'memory limit exceeded: 2 bytes > 1 bytes')
  t.is(collector.output, '')
})

test('CollectStreams charges UTF-8 multi-byte characters', () => {
  // '😀' is 2 UTF-16 code units but 4 UTF-8 bytes; +64 overhead → 68.
  const collector = new CollectStreams(5)
  const thrown = t.throws<MontyRuntimeError>(() => collector.write('stdout', '😀'))
  t.true(thrown instanceof MontyRuntimeError)
  t.is(thrown.exception.typeName, 'MemoryError')
  t.is(thrown.exception.message, 'memory limit exceeded: 68 bytes > 5 bytes')
  t.deepEqual(collector.output, [])
})

test('CollectString reuses across feeds', async () => {
  const collector = new CollectString()
  const session = await pool().checkout()
  try {
    await session.feedRun('print("first")', { printCallback: collector })
    await session.feedRun('print("second")', { printCallback: collector })
  } finally {
    await session.close()
  }
  t.is(collector.output, 'first\nsecond\n')
})

test('CollectString/CollectStreams reject invalid maxBytes', () => {
  for (const bad of [NaN, Infinity, -Infinity, -1] as const) {
    const msg = t.throws(() => new CollectString(bad)).message
    t.is(msg, 'maxBytes must be a finite non-negative number or null')
    t.is(t.throws(() => new CollectStreams(bad)).message, msg)
  }
  // null still opts out
  const unlimited = new CollectString(null)
  unlimited.write('stdout', 'x'.repeat(200))
  t.is(unlimited.output.length, 200)
})

test('print collect cap fails before feedStart returns a snapshot', async () => {
  // print then suspend: MemoryError must surface immediately, not after resume.
  const collector = new CollectString(10)
  const session = await pool().checkout()
  try {
    const thrown = await t.throwsAsync<MontyRuntimeError>(() =>
      session.feedStart("print('x' * 100)\nfetch()", {
        printCallback: collector,
        externalLookup: { fetch: () => 1 },
      }),
    )
    t.true(thrown instanceof MontyRuntimeError)
    t.is(thrown.exception.typeName, 'MemoryError')
    t.true(thrown.exception.message.startsWith('memory limit exceeded:'))
    // session poisoned (worker left suspended with no resume)
    const next = await t.throwsAsync(() => session.feedRun('1 + 1'))
    t.true(next instanceof MontyRuntimeError)
    t.is((next as MontyRuntimeError).exception.typeName, 'MemoryError')
  } finally {
    await session.close()
  }
})

test('print collect cap fails before feedRun answers a suspension', async () => {
  const collector = new CollectString(10)
  const session = await pool().checkout()
  try {
    const thrown = await t.throwsAsync<MontyRuntimeError>(() =>
      session.feedRun("print('x' * 100)\nfetch()", {
        printCallback: collector,
        externalLookup: { fetch: () => 1 },
      }),
    )
    t.true(thrown instanceof MontyRuntimeError)
    t.is(thrown.exception.typeName, 'MemoryError')
    const next = await t.throwsAsync(() => session.feedRun('1 + 1'))
    t.true(next instanceof MontyRuntimeError)
  } finally {
    await session.close()
  }
})
