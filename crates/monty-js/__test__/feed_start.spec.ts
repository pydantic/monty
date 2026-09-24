import { ROOT_CONTEXT, context, createContextKey, propagation } from '@opentelemetry/api'
import { test, vi } from 'vitest'
import { t } from './assertions.js'

import { FunctionSnapshot, FutureSnapshot, MontyComplete, MontyRuntimeError, NameLookupSnapshot } from '@pydantic/monty'
import { MountDir } from '@pydantic/monty/node'
import { kind } from './env.js'
import { setupPool } from './helpers.js'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const { pool } = setupPool()

test('feedStart suspends at a function call, then completes', async () => {
  const session = await pool().checkout()
  try {
    const snap = await session.feedStart('x = add(2, 3)\nx * 10')
    t.true(snap instanceof FunctionSnapshot)
    const call = snap as FunctionSnapshot
    t.is(call.functionName, 'add')
    t.deepEqual(call.args, [2, 3])
    t.false(call.isOsFunction)
    const done = await call.resume(5)
    t.true(done instanceof MontyComplete)
    t.is((done as MontyComplete).output, 50)
  } finally {
    await session.close()
  }
})

test('feedStart surfaces a name lookup', async () => {
  const session = await pool().checkout()
  try {
    const snap = await session.feedStart('missing + 1')
    t.true(snap instanceof NameLookupSnapshot)
    t.is((snap as NameLookupSnapshot).variableName, 'missing')
  } finally {
    await session.close()
  }
})

test('snapshot trace contexts preserve the captured context without Monty tracing', async () => {
  const key = createContextKey('snapshot context')
  const [feedContext, loadContext, callerContext] = ['feed', 'load', 'caller'].map((value) =>
    propagation.setBaggage(ROOT_CONTEXT, propagation.createBaggage({ request: { value } })).setValue(key, value),
  )
  const session = await pool().checkout()
  // Control the active context without a Node-only async context manager.
  const active = vi.spyOn(context, 'active').mockReturnValue(feedContext)
  try {
    const pendingName = session.feedStart('missing')
    active.mockReturnValue(callerContext)
    const name = (await pendingName) as NameLookupSnapshot
    t.is(name.traceContext(), feedContext)
    t.is(name.traceContext().getValue(key), 'feed')
    t.deepEqual(propagation.getBaggage(name.traceContext())?.getEntry('request'), { value: 'feed' })
    t.is(context.active(), callerContext)
    const dump = await name.dump()
    await name.resumeValue(42)
    t.throws(() => name.traceContext(), { message: 'snapshot has already been resumed' })

    active.mockReturnValue(feedContext)
    const pendingCall = session.feedStart('await callback()')
    active.mockReturnValue(callerContext)
    const call = (await pendingCall) as FunctionSnapshot
    t.is(call.traceContext(), feedContext)
    const futures = (await call.resumeFuture()) as FutureSnapshot
    t.throws(() => call.traceContext(), { message: 'snapshot has already been resumed' })
    t.is(futures.traceContext(), feedContext)
    await futures.resume([{ callId: call.callId, value: 42 }])
    t.throws(() => futures.traceContext(), { message: 'snapshot has already been resumed' })

    const restoredSession = await pool().checkout()
    try {
      active.mockReturnValue(loadContext)
      const pendingRestore = restoredSession.loadSnapshot(dump)
      active.mockReturnValue(callerContext)
      const restored = (await pendingRestore) as NameLookupSnapshot
      t.is(restored.traceContext(), loadContext)
      t.is(restored.traceContext().getValue(key), 'load')
      t.deepEqual(propagation.getBaggage(restored.traceContext())?.getEntry('request'), { value: 'load' })
      t.is(context.active(), callerContext)
      await restored.resumeValue(42)
    } finally {
      await restoredSession.close()
    }
  } finally {
    active.mockRestore()
    await session.close()
  }
})

test('a snapshot resumes at most once', async () => {
  const session = await pool().checkout()
  try {
    const snap = (await session.feedStart('f()')) as FunctionSnapshot
    await snap.resume(1)
    t.throws(() => snap.resume(2), { message: 'snapshot has already been resumed' })
  } finally {
    await session.close()
  }
})

test('os handler is used by resumeAuto, not auto-dispatched', async () => {
  const session = await pool().checkout()
  try {
    const snap = await session.feedStart("from pathlib import Path\nPath('/data/x').read_text()", {
      os: (name) => {
        t.is(name, 'Path.read_text')
        return 'file body'
      },
    })
    // feedStart surfaces every OS call; the handler only backs resumeAuto
    t.true(snap instanceof FunctionSnapshot)
    t.true((snap as FunctionSnapshot).isOsFunction)
    const done = (await (snap as FunctionSnapshot).resumeAuto()) as MontyComplete
    t.true(done instanceof MontyComplete)
    t.is(done.output, 'file body')
  } finally {
    await session.close()
  }
})

test('resumeAuto settles an immediately awaited asyncio.sleep in place', async () => {
  const session = await pool().checkout({ osPolicy: { sleep: 'call_host' } })
  try {
    const snap = await session.feedStart("import asyncio\nawait asyncio.sleep(0.001, 'woken')", {
      os: async (name, args) => {
        t.is(name, 'asyncio.sleep')
        await new Promise((resolve) => setTimeout(resolve, (args[0] as number) * 1000))
      },
    })
    t.true(snap instanceof FunctionSnapshot)
    t.true((snap as FunctionSnapshot).allowEagerAwait)
    // no FutureSnapshot in between: the wait is done when the answer arrives
    const done = (await (snap as FunctionSnapshot).resumeAuto()) as MontyComplete
    t.true(done instanceof MontyComplete)
    t.is(done.output, 'woken')
  } finally {
    await session.close()
  }
})

test('the sandbox future mechanism is caller-driven', async () => {
  const session = await pool().checkout()
  try {
    const code = 'import asyncio\nasync def main():\n    return await go()\nasyncio.run(main())'
    const call = (await session.feedStart(code)) as FunctionSnapshot
    t.is(call.functionName, 'go')
    const futures = (await call.resumeFuture()) as FutureSnapshot
    t.true(futures instanceof FutureSnapshot)
    t.deepEqual(futures.pendingCallIds, [call.callId])
    const done = (await futures.resume([{ callId: call.callId, value: 99 }])) as MontyComplete
    t.true(done instanceof MontyComplete)
    t.is(done.output, 99)
  } finally {
    await session.close()
  }
})

test('every snapshot kind carries the position of the suspending expression', async () => {
  const session = await pool().checkout()
  try {
    const call = (await session.feedStart('x = 1\ny = add(x, 2) + 1')) as FunctionSnapshot
    t.true(call instanceof FunctionSnapshot)
    t.deepEqual(call.position, { filename: '<python-input-0>', start: 10, end: 19 })
    await call.resume(3)

    const name = (await session.feedStart('total = 1 + missing')) as NameLookupSnapshot
    t.true(name instanceof NameLookupSnapshot)
    t.deepEqual(name.position, { filename: '<python-input-1>', start: 12, end: 19 })
    await name.resumeValue(1)

    const osCall = (await session.feedStart("from pathlib import Path\nPath('/etc/x').read_text()")) as FunctionSnapshot
    t.true(osCall.isOsFunction)
    t.deepEqual(osCall.position, { filename: '<python-input-2>', start: 25, end: 51 })
    await osCall.resume('body')

    const code = 'import asyncio\n\nasync def go():\n    return await fetch()\n\nawait asyncio.gather(go(), go())'
    const first = (await session.feedStart(code)) as FunctionSnapshot
    t.deepEqual(first.position, { filename: '<python-input-3>', start: 49, end: 56 })
    const second = (await first.resumeFuture()) as FunctionSnapshot
    const futures = (await second.resumeFuture()) as FutureSnapshot
    t.true(futures instanceof FutureSnapshot)
    t.deepEqual(futures.position, { filename: '<python-input-3>', start: 58, end: 90 })
    await futures.resume([
      { callId: first.callId, value: 1 },
      { callId: second.callId, value: 2 },
    ])
  } finally {
    await session.close()
  }
})

test('the position counts UTF-8 bytes, not characters', async () => {
  const session = await pool().checkout()
  try {
    // the two-byte `é` puts the call at byte 13 but character 12
    const code = "x = 'é'\ny = add(x, 2)"
    const call = (await session.feedStart(code)) as FunctionSnapshot
    t.deepEqual(call.position, { filename: '<python-input-0>', start: 13, end: 22 })
    const bytes = new TextEncoder().encode(code).subarray(call.position.start, call.position.end)
    t.is(new TextDecoder().decode(bytes), 'add(x, 2)')
    await call.resume(3)
  } finally {
    await session.close()
  }
})

test('the position survives dump and loadSnapshot', async () => {
  let blob: Buffer
  {
    const session = await pool().checkout()
    const snap = (await session.feedStart('y = fetch()\ny + 1')) as FunctionSnapshot
    blob = await snap.dump()
    await session.close()
  }
  const session = await pool().checkout()
  try {
    const snap = (await session.loadSnapshot(blob)) as FunctionSnapshot
    t.deepEqual(snap.position, { filename: '<python-input-0>', start: 4, end: 11 })
  } finally {
    await session.close()
  }
})

test('dump at a suspension, then loadSnapshot and resume', async () => {
  let blob: Buffer
  {
    const session = await pool().checkout()
    const snap = (await session.feedStart('y = fetch()\ny + 1')) as FunctionSnapshot
    blob = await snap.dump()
    await session.close()
  }
  const session = await pool().checkout()
  try {
    const snap = await session.loadSnapshot(blob)
    t.true(snap instanceof FunctionSnapshot)
    const done = (await (snap as FunctionSnapshot).resume(41)) as MontyComplete
    t.is(done.output, 42)
  } finally {
    await session.close()
  }
})

test('loadSession restores an idle session', async () => {
  let blob: Buffer
  {
    const session = await pool().checkout()
    await session.feedRun('kept = 7')
    blob = await session.dump()
    await session.close()
  }
  const session = await pool().checkout()
  try {
    await session.loadSession(blob)
    t.is(await session.feedRun('kept + 1'), 8)
  } finally {
    await session.close()
  }
})

test('loadSession and loadSnapshot reject the wrong dump kind', async () => {
  let idle: Buffer
  let suspended: Buffer
  {
    const session = await pool().checkout()
    idle = await session.dump()
    await session.close()
  }
  {
    const session = await pool().checkout()
    await session.feedStart('f()')
    suspended = await session.dump()
    await session.close()
  }
  {
    const session = await pool().checkout()
    await t.throwsAsync(() => session.loadSnapshot(idle), {
      message: 'this dump is an idle session — use loadSession() to restore it',
    })
    // the failed load poisons the session — it is not retryable
    await t.throwsAsync(() => session.feedRun('1 + 1'))
    await session.close()
  }
  {
    const session = await pool().checkout()
    await t.throwsAsync(() => session.loadSession(suspended), {
      message: 'this dump is a suspended snapshot — use loadSnapshot() to resume it',
    })
    await t.throwsAsync(() => session.feedRun('1 + 1'))
    await session.close()
  }
})

test('load after a feed is rejected', async () => {
  const session = await pool().checkout()
  try {
    const blob = await session.dump()
    await session.feedRun('x = 1')
    await t.throwsAsync(() => session.loadSnapshot(blob), {
      message:
        'loadSession / loadSnapshot is only valid on a fresh session, before any feedRun / feedStart / loadSession / loadSnapshot',
    })
  } finally {
    await session.close()
  }
})

test('mounts are re-supplied to loadSnapshot', async () => {
  if (kind === 'browser') {
    const session = await pool().checkout()
    try {
      const snap = (await session.feedStart('f()')) as FunctionSnapshot
      const blob = await snap.dump()
      const restore = await pool().checkout()
      try {
        const error = await t.throwsAsync(() => restore.loadSnapshot(blob, { mount: [{}] as never }))
        t.is(error.message, 'the wasm worker does not support filesystem mounts (browser has no host filesystem)')
      } finally {
        await restore.close()
      }
    } finally {
      await session.close()
    }
    return
  }

  const dir = await mkdtemp(join(tmpdir(), 'monty-js-snap-'))
  await writeFile(join(dir, 'hello.txt'), 'hi')
  const mount = new MountDir({ hostPath: dir, virtualPath: '/data', mode: 'read-only' })
  const code = "f()\nfrom pathlib import Path\nPath('/data/hello.txt').read_text()"

  let blob: Buffer
  {
    const session = await pool().checkout()
    const snap = (await session.feedStart(code, { mount })) as FunctionSnapshot
    blob = await snap.dump()
    await session.close()
  }

  // re-supplied: the mounted read surfaces, and resumeAuto serves it from the
  // rebuilt mount table
  {
    const session = await pool().checkout()
    const snap = (await session.loadSnapshot(blob, { mount })) as FunctionSnapshot
    const read = (await snap.resume(null)) as FunctionSnapshot
    t.true(read.isOsFunction)
    const done = (await read.resumeAuto()) as MontyComplete
    t.is(done.output, 'hi')
    await session.close()
  }

  // omitted: nothing validates the re-supply (mounts are never part of the
  // dump) — the resumed feed's mounted read degrades into a surfaced OS call,
  // and leaving it unhandled raises PermissionError inside the sandbox
  {
    const session = await pool().checkout()
    const snap = (await session.loadSnapshot(blob)) as FunctionSnapshot
    const osSnap = (await snap.resume(null)) as FunctionSnapshot
    t.true(osSnap.isOsFunction)
    t.is(osSnap.functionName, 'Path.read_text')
    const error = await t.throwsAsync(() => osSnap.resumeNotHandled(), { instanceOf: MontyRuntimeError })
    t.is(error.message, "PermissionError: Permission denied: '/data/hello.txt'")
    await session.close()
  }
})

// =============================================================================
// resumeAuto: answer each suspension from the captured externalLookup / os
// =============================================================================

test('resumeAuto answers a function call from externalLookup', async () => {
  const session = await pool().checkout()
  try {
    const snap = (await session.feedStart('add(2, 3) * 10', {
      externalLookup: { add: (a: number, b: number) => a + b },
    })) as FunctionSnapshot
    const done = (await snap.resumeAuto()) as MontyComplete
    t.true(done instanceof MontyComplete)
    t.is(done.output, 50)
  } finally {
    await session.close()
  }
})

test('resumeAuto drives a snippet to completion', async () => {
  const session = await pool().checkout()
  try {
    // mixes a name lookup (`base`) and two external calls (`add`)
    const code = 'total = base\nfor i in [1, 2]:\n    total = add(total, i)\ntotal'
    const externalLookup = { base: 10, add: (a: number, b: number) => a + b }
    let snap = await session.feedStart(code, { externalLookup })
    let steps = 0
    while (!(snap instanceof MontyComplete)) {
      snap = await snap.resumeAuto()
      steps++
    }
    t.is(snap.output, 13)
    t.is(steps, 3)
  } finally {
    await session.close()
  }
})

test('resumeAuto resolves a name lookup to a value', async () => {
  const session = await pool().checkout()
  try {
    const snap = (await session.feedStart('missing + 1', { externalLookup: { missing: 41 } })) as NameLookupSnapshot
    const done = (await snap.resumeAuto()) as MontyComplete
    t.is(done.output, 42)
  } finally {
    await session.close()
  }
})

test('resumeAuto resolves a name lookup to a function', async () => {
  const session = await pool().checkout()
  try {
    // `greet` is read as a value (name lookup), then the bound name is called
    let snap = await session.feedStart('g = greet\ng("hi")', {
      externalLookup: { greet: (s: string) => s + '!' },
    })
    while (!(snap instanceof MontyComplete)) {
      snap = await snap.resumeAuto()
    }
    t.is(snap.output, 'hi!')
  } finally {
    await session.close()
  }
})

test('resumeAuto with a missing name raises NameError', async () => {
  const session = await pool().checkout()
  try {
    const snap = (await session.feedStart('missing + 1', { externalLookup: {} })) as NameLookupSnapshot
    const error = await t.throwsAsync(() => snap.resumeAuto(), { instanceOf: MontyRuntimeError })
    t.is(error.message, "NameError: name 'missing' is not defined")
  } finally {
    await session.close()
  }
})

test('resumeAuto with a function absent from the lookup raises NameError', async () => {
  const session = await pool().checkout()
  try {
    const snap = (await session.feedStart('add(2, 3)', { externalLookup: {} })) as FunctionSnapshot
    const error = await t.throwsAsync(() => snap.resumeAuto(), { instanceOf: MontyRuntimeError })
    t.is(error.message, "NameError: name 'add' is not defined")
  } finally {
    await session.close()
  }
})

test('resumeAuto answers an OS call with the default unhandled error', async () => {
  const session = await pool().checkout()
  try {
    // no os handler was captured, so resumeAuto answers with monty's default
    // unhandled-OS error, which the snippet catches
    const code = [
      'from pathlib import Path',
      'try:',
      "    Path('/etc/secret').read_text()",
      "    r = 'unexpected'",
      'except Exception as e:',
      '    r = type(e).__name__',
      'r',
    ].join('\n')
    const snap = (await session.feedStart(code)) as FunctionSnapshot
    t.true(snap.isOsFunction)
    const done = (await snap.resumeAuto()) as MontyComplete
    t.is(done.output, 'PermissionError')
  } finally {
    await session.close()
  }
})

test('resumeAuto settles an immediately awaited promise without a FutureSnapshot', async () => {
  const session = await pool().checkout()
  try {
    const code = 'import asyncio\nasync def main():\n    return await go()\nasyncio.run(main())'
    const snap = (await session.feedStart(code, { externalLookup: { go: async () => 99 } })) as FunctionSnapshot
    t.true(snap.allowEagerAwait)
    const done = (await snap.resumeAuto()) as MontyComplete
    t.is(done.output, 99)
  } finally {
    await session.close()
  }
})

test('resumeAuto drives multiple pending promises via gather', async () => {
  const session = await pool().checkout()
  try {
    const code = 'import asyncio\nasync def main():\n    return await asyncio.gather(go(1), go(2))\nasyncio.run(main())'
    let snap = await session.feedStart(code, { externalLookup: { go: async (n: number) => n * 10 } })
    while (!(snap instanceof MontyComplete)) {
      snap = await snap.resumeAuto()
    }
    t.deepEqual(snap.output, [10, 20])
  } finally {
    await session.close()
  }
})

test('resumeAuto and manual resume share the captured lookup', async () => {
  const session = await pool().checkout()
  try {
    const code = 'a = first()\nb = second()\na + b'
    const snap = (await session.feedStart(code, { externalLookup: { second: () => 20 } })) as FunctionSnapshot
    t.is(snap.functionName, 'first')
    const next = (await snap.resume(5)) as FunctionSnapshot
    t.is(next.functionName, 'second')
    const done = (await next.resumeAuto()) as MontyComplete
    t.is(done.output, 25)
  } finally {
    await session.close()
  }
})

test('resumeAuto resumes at most once', async () => {
  const session = await pool().checkout()
  try {
    const snap = (await session.feedStart('add(1, 2)', {
      externalLookup: { add: (a: number, b: number) => a + b },
    })) as FunctionSnapshot
    await snap.resumeAuto()
    t.throws(() => snap.resumeAuto(), { message: 'snapshot has already been resumed' })
  } finally {
    await session.close()
  }
})

test('loadSnapshot captures externalLookup for resumeAuto', async () => {
  let blob: Buffer
  {
    const session = await pool().checkout()
    const snap = (await session.feedStart('y = fetch()\ny + 1')) as FunctionSnapshot
    blob = await snap.dump()
    await session.close()
  }
  const session = await pool().checkout()
  try {
    const snap = (await session.loadSnapshot(blob, { externalLookup: { fetch: () => 41 } })) as FunctionSnapshot
    const done = (await snap.resumeAuto()) as MontyComplete
    t.is(done.output, 42)
  } finally {
    await session.close()
  }
})
