import { test } from 'vitest'
import { t } from './assertions.js'
import { kind } from './env.js'

import { MontyRuntimeError, type ResourceLimits } from '@pydantic/monty'
import { setupPool } from './helpers.js'

const { run } = setupPool()

const isRuntimeError = { instanceOf: MontyRuntimeError }

// =============================================================================
// ResourceLimits construction tests
// =============================================================================

test('resource limits custom', async () => {
  const limits: ResourceLimits = {
    maxAllocations: 100,
    maxDurationSecs: 5.0,
    maxMemory: 1024,
    gcInterval: 10,
    maxRecursionDepth: 500,
  }
  // Just verify the object is valid and can be passed
  t.is(await run('1 + 1', { limits }), 2)
})

test('run with limits', async () => {
  t.is(await run('1 + 1', { limits: { maxDurationSecs: 5.0 } }), 2)
})

// =============================================================================
// Recursion limit tests
// =============================================================================

test('recursion limit', async () => {
  const code = `
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(10)
`
  const error = await t.throwsAsync(() => run(code, { limits: { maxRecursionDepth: 5 } }), isRuntimeError)
  t.is(error.message, 'RecursionError: maximum recursion depth exceeded')
})

test('recursion limit ok', async () => {
  const code = `
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(5)
`
  t.is(await run(code, { limits: { maxRecursionDepth: 100 } }), 5)
})

// =============================================================================
// Allocation limit tests
// =============================================================================

test('allocation limit', async () => {
  // Use a more aggressive allocation pattern
  const code = `
result = []
for i in range(10000):
    result.append([i])
len(result)
`
  const error = await t.throwsAsync(() => run(code, { limits: { maxAllocations: 5 } }), isRuntimeError)
  t.is(error.message, 'MemoryError: allocation limit exceeded: 6 > 5')
})

test('allocation limit accepts values above u32 max', async () => {
  t.is(await run('1 + 1', { limits: { maxAllocations: 2 ** 33 } }), 2)
})

// =============================================================================
// Memory limit tests
// =============================================================================

test('memory limit', async () => {
  const code = `
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
`
  const error = await t.throwsAsync(() => run(code, { limits: { maxMemory: 100 } }), isRuntimeError)
  t.is(
    error.message,
    kind === 'browser'
      ? 'MemoryError: memory limit exceeded: 180 bytes > 100 bytes'
      : 'MemoryError: memory limit exceeded: 120 bytes > 100 bytes',
  )
})

test('memory limit accepts values above u32 max', async () => {
  t.is(await run('1 + 1', { limits: { maxMemory: 2 ** 33 } }), 2)
})

// =============================================================================
// Limits with inputs tests
// =============================================================================

test('limits with inputs', async () => {
  t.is(await run('x * 2', { inputs: { x: 21 }, limits: { maxDurationSecs: 5.0 } }), 42)
})

// =============================================================================
// Large operation limits tests
// =============================================================================

test('pow memory limit', async () => {
  const error = await t.throwsAsync(() => run('2 ** 10000000', { limits: { maxMemory: 1_000_000 } }), isRuntimeError)
  t.is(error.message, 'MemoryError: memory limit exceeded: 10000000 bytes > 1000000 bytes')
})

test('lshift memory limit', async () => {
  const error = await t.throwsAsync(() => run('1 << 10000000', { limits: { maxMemory: 1_000_000 } }), isRuntimeError)
  t.is(error.message, 'MemoryError: memory limit exceeded: 1250001 bytes > 1000000 bytes')
})

test('mult memory limit', async () => {
  const code = `
big = 2 ** 4000000
result = big * big
`
  const error = await t.throwsAsync(() => run(code, { limits: { maxMemory: 1_000_000 } }), isRuntimeError)
  t.is(error.message, 'MemoryError: memory limit exceeded: 4000000 bytes > 1000000 bytes')
})

test('small operations within limit', async () => {
  const result = await run('2 ** 1000', { limits: { maxMemory: 1_000_000 } })
  t.is(typeof result, 'bigint')
  t.is(result, 2n ** 1000n)
})

// =============================================================================
// Time limit tests
// =============================================================================

test('time limit', async () => {
  const error = await t.throwsAsync(
    () => run('while True:\n    pass\n', { limits: { maxDurationSecs: 0.1 } }),
    isRuntimeError,
  )
  t.is(error.exception.typeName, 'TimeoutError')
  // The reported elapsed time varies from run to run; the limit is fixed.
  t.regex(error.display('msg'), /^time limit exceeded: \d+(\.\d+)?ms > 100ms$/)
})
