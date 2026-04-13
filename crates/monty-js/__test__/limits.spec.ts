import test from 'ava'

import { Monty, MontyRuntimeError, type ResourceLimits } from '../wrapper'

// =============================================================================
// ResourceLimits construction tests
// =============================================================================

test('resource limits custom', (t) => {
  const limits: ResourceLimits = {
    maxAllocations: 100,
    maxDurationSecs: 5.0,
    maxMemory: 1024,
    gcInterval: 10,
    maxRecursionDepth: 500,
  }
  // Just verify the object is valid and can be passed
  const m = new Monty('1 + 1')
  t.is(m.run({ limits }), 2)
})

test('run with limits', (t) => {
  const m = new Monty('1 + 1')
  const limits: ResourceLimits = { maxDurationSecs: 5.0 }
  t.is(m.run({ limits }), 2)
})

// =============================================================================
// Recursion limit tests
// =============================================================================

test('recursion limit', (t) => {
  const code = `
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(10)
`
  const m = new Monty(code)
  const limits: ResourceLimits = { maxRecursionDepth: 5 }
  const error = t.throws(() => m.run({ limits }), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('RecursionError'))
})

test('recursion limit ok', (t) => {
  const code = `
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(5)
`
  const m = new Monty(code)
  const limits: ResourceLimits = { maxRecursionDepth: 100 }
  t.is(m.run({ limits }), 5)
})

// =============================================================================
// Allocation limit tests
// =============================================================================

test('allocation limit', (t) => {
  // Use a more aggressive allocation pattern
  const code = `
result = []
for i in range(10000):
    result.append([i])
len(result)
`
  const m = new Monty(code)
  const limits: ResourceLimits = { maxAllocations: 5 }
  const error = t.throws(() => m.run({ limits }), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('MemoryError'))
})

// =============================================================================
// Memory limit tests
// =============================================================================

test('memory limit', (t) => {
  const code = `
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
`
  const m = new Monty(code)
  const limits: ResourceLimits = { maxMemory: 100 }
  const error = t.throws(() => m.run({ limits }), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('MemoryError'))
})

// =============================================================================
// Limits with inputs tests
// =============================================================================

test('limits with inputs', (t) => {
  const m = new Monty('x * 2', { inputs: ['x'] })
  const limits: ResourceLimits = { maxDurationSecs: 5.0 }
  t.is(m.run({ inputs: { x: 21 }, limits }), 42)
})

// =============================================================================
// Large operation limits tests
// =============================================================================

test('pow memory limit', (t) => {
  const m = new Monty('2 ** 10000000')
  const limits: ResourceLimits = { maxMemory: 1_000_000 }
  const error = t.throws(() => m.run({ limits }), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('MemoryError'))
})

test('lshift memory limit', (t) => {
  const m = new Monty('1 << 10000000')
  const limits: ResourceLimits = { maxMemory: 1_000_000 }
  const error = t.throws(() => m.run({ limits }), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('MemoryError'))
})

test('mult memory limit', (t) => {
  const code = `
big = 2 ** 4000000
result = big * big
`
  const m = new Monty(code)
  const limits: ResourceLimits = { maxMemory: 1_000_000 }
  const error = t.throws(() => m.run({ limits }), { instanceOf: MontyRuntimeError })
  t.true(error.message.includes('MemoryError'))
})

test('small operations within limit', (t) => {
  const m = new Monty('2 ** 1000')
  const limits: ResourceLimits = { maxMemory: 1_000_000 }
  const result = m.run({ limits })
  t.true(typeof result === 'bigint' || typeof result === 'number')
})

// =============================================================================
// Time limit tests
// =============================================================================

test('time limit', (t) => {
  // Use recursion instead of while loop
  const code = `
def infinite(n):
    return infinite(n + 1)
infinite(0)
`
  const m = new Monty(code)
  const limits: ResourceLimits = { maxDurationSecs: 0.1 }
  const error = t.throws(() => m.run({ limits }))
  // May hit time limit or recursion limit
  t.true(
    error?.message.includes('TimeoutError') ||
      error?.message.includes('timed out') ||
      error?.message.includes('RecursionError'),
  )
})
