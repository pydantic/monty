import { test } from 'vitest'
import { assertMemoryError, t } from './assertions.js'
import { kind } from './env.js'

import { MontyRuntimeError, type ResourceLimits } from '@pydantic/monty'
import { WorkerTransport } from '../ts/worker/transport.js'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

const isRuntimeError = { instanceOf: MontyRuntimeError }

// =============================================================================
// ResourceLimits construction tests
// =============================================================================

test('resource limits custom', async () => {
  const limits: ResourceLimits = {
    maxDurationSecs: 5.0,
    maxMemory: 64 * 1024,
    gcInterval: 10,
    maxRecursionDepth: 500,
    maxSuspensions: 20,
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
// Memory limit tests
// =============================================================================

test('memory limit', async () => {
  const code = `
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
`
  const maxMemory = 64 * 1024
  const error = await t.throwsAsync(() => run(code, { limits: { maxMemory } }), isRuntimeError)
  assertMemoryError(error, kind === 'browser' ? 75_047 : 89_113, maxMemory)
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
  assertMemoryError(error, kind === 'browser' ? 10_023_470 : 10_031_312, 1_000_000)
})

test('lshift memory limit', async () => {
  const error = await t.throwsAsync(() => run('1 << 10000000', { limits: { maxMemory: 1_000_000 } }), isRuntimeError)
  assertMemoryError(error, kind === 'browser' ? 1_273_471 : 1_281_313, 1_000_000)
})

test('mult memory limit', async () => {
  const code = `
big = 2 ** 4000000
result = big * big
`
  const error = await t.throwsAsync(() => run(code, { limits: { maxMemory: 1_000_000 } }), isRuntimeError)
  assertMemoryError(error, kind === 'browser' ? 4_024_130 : 4_031_972, 1_000_000)
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

// =============================================================================
// Suspension limit tests
// =============================================================================

test('suspension limit', async () => {
  // Pool enforcement keeps sandboxed exception handling from retrying forever.
  const code = `
n = 0
while True:
    try:
        fetch('x')
    except Exception:
        n += 1
`
  const fetch = () => {
    throw new Error('refused')
  }
  const error = await t.throwsAsync(
    () => run(code, { limits: { maxSuspensions: 3 }, externalLookup: { fetch } }),
    isRuntimeError,
  )
  t.is(error.exception.typeName, 'RuntimeError')
  t.is(error.display('msg'), 'suspension limit 3 exceeded')
})

test('suspension limit defaults to 1000', async () => {
  await using session = await pool().checkout()
  const error = await t.throwsAsync(
    () => session.feedRun('n = 0\nwhile True:\n    fetch()\n    n += 1', { externalLookup: { fetch: () => null } }),
    isRuntimeError,
  )
  t.is(error.display('msg'), 'suspension limit 1000 exceeded')
  t.is(await session.feedRun('n'), 1000)
})

test('suspension limit leaves the session usable', async () => {
  await using session = await pool().checkout({ limits: { maxSuspensions: 1 } })
  const fetch = () => 'ok'
  t.is(await session.feedRun("fetch('x')", { externalLookup: { fetch } }), 'ok')
  const error = await t.throwsAsync(() => session.feedRun("fetch('y')", { externalLookup: { fetch } }), isRuntimeError)
  t.is(error.display('msg'), 'suspension limit 1 exceeded')
  t.is(await session.feedRun('1 + 1'), 2)
})

test('a suspension answering abort-feed ends the wasm worker', async () => {
  // A compromised component could answer the abort with another suspension;
  // servicing it would let it call host functions past the budget.
  const call = (callId: number) => ({
    tag: 'function-call' as const,
    val: { callId, functionName: 'fetch', args: [], kwargs: [] },
  })
  const requests: string[] = []
  const transport = await WorkerTransport.create(async (request) => {
    requests.push(request.tag)
    return request.tag === 'configure'
      ? { status: 'continue', events: [{ tag: 'ok' }], maxSuspensions: 1n }
      : { status: 'continue', events: [call(requests.length)] }
  })
  let reusable: boolean | undefined
  transport.onFinish = (value) => {
    reusable = value
  }
  const first = await transport.feed('fetch()', null, [], { skipTypeCheck: true }, () => {})
  t.is(first.kind, 'functionCall')
  const turn = await transport.resumeReturn(null, () => {})
  t.deepEqual(turn, { kind: 'protocol', message: 'worker answered abort-feed with functionCall' })
  t.deepEqual(requests, ['configure', 'feed', 'resume-call', 'abort-feed'])
  await transport.finish()
  t.is(reusable, false)
})

test('restored session keeps its suspension limit with a fresh count', async () => {
  const fetch = () => 'ok'
  let state: Buffer
  {
    await using session = await pool().checkout({ limits: { maxSuspensions: 1 } })
    t.is(await session.feedRun("fetch('x')", { externalLookup: { fetch } }), 'ok')
    state = await session.dump()
  }

  await using restored = await pool().checkout()
  await restored.loadSession(state)
  t.is(await restored.feedRun("fetch('y')", { externalLookup: { fetch } }), 'ok')
  const error = await t.throwsAsync(() => restored.feedRun("fetch('z')", { externalLookup: { fetch } }), isRuntimeError)
  t.is(error.display('msg'), 'suspension limit 1 exceeded')
})
