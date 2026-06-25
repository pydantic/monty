import test from 'ava'

import {
  FunctionSnapshot,
  FutureSnapshot,
  MontyComplete,
  NameLookupSnapshot,
  MontyRuntimeError,
  MountDir,
} from '../ts/index.js'
import { setupPool } from './helpers.js'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const { pool } = setupPool(test)

test('feedStart suspends at a function call, then completes', async (t) => {
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

test('feedStart surfaces a name lookup', async (t) => {
  const session = await pool().checkout()
  try {
    const snap = await session.feedStart('missing + 1')
    t.true(snap instanceof NameLookupSnapshot)
    t.is((snap as NameLookupSnapshot).variableName, 'missing')
  } finally {
    await session.close()
  }
})

test('a snapshot resumes at most once', async (t) => {
  const session = await pool().checkout()
  try {
    const snap = (await session.feedStart('f()')) as FunctionSnapshot
    await snap.resume(1)
    t.throws(() => snap.resume(2), { message: 'snapshot has already been resumed' })
  } finally {
    await session.close()
  }
})

test('os handler is auto-dispatched between snapshots', async (t) => {
  const session = await pool().checkout()
  try {
    const snap = await session.feedStart("from pathlib import Path\nPath('/data/x').read_text()", {
      os: (name) => {
        t.is(name, 'Path.read_text')
        return 'file body'
      },
    })
    t.true(snap instanceof MontyComplete)
    t.is((snap as MontyComplete).output, 'file body')
  } finally {
    await session.close()
  }
})

test('the sandbox future mechanism is caller-driven', async (t) => {
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

test('dump at a suspension, then loadSnapshot and resume', async (t) => {
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

test('load restores an idle session', async (t) => {
  let blob: Buffer
  {
    const session = await pool().checkout()
    await session.feedRun('kept = 7')
    blob = await session.dump()
    await session.close()
  }
  const session = await pool().checkout()
  try {
    await session.load(blob)
    t.is(await session.feedRun('kept + 1'), 8)
  } finally {
    await session.close()
  }
})

test('load and loadSnapshot reject the wrong dump kind', async (t) => {
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
      message: 'this dump is an idle session — use load() to restore it',
    })
    // the failed load poisons the session — it is not retryable
    await t.throwsAsync(() => session.feedRun('1 + 1'))
    await session.close()
  }
  {
    const session = await pool().checkout()
    await t.throwsAsync(() => session.load(suspended), {
      message: 'this dump is a suspended snapshot — use loadSnapshot() to resume it',
    })
    await t.throwsAsync(() => session.feedRun('1 + 1'))
    await session.close()
  }
})

test('load after a feed is rejected', async (t) => {
  const session = await pool().checkout()
  try {
    const blob = await session.dump()
    await session.feedRun('x = 1')
    await t.throwsAsync(() => session.loadSnapshot(blob), {
      message:
        'load / loadSnapshot is only valid on a fresh session, before any feedRun / feedStart / load / loadSnapshot',
    })
  } finally {
    await session.close()
  }
})

test('mounts are re-supplied to loadSnapshot and validated', async (t) => {
  const dir = await mkdtemp(join(tmpdir(), 'monty-js-snap-'))
  await writeFile(join(dir, 'hello.txt'), 'hi')
  const mount = new MountDir('/data', dir, { mode: 'read-only' })
  const code = "f()\nfrom pathlib import Path\nPath('/data/hello.txt').read_text()"

  let blob: Buffer
  {
    const session = await pool().checkout()
    const snap = (await session.feedStart(code, { mount })) as FunctionSnapshot
    blob = await snap.dump()
    await session.close()
  }

  // re-supplied: the mounted read is served and execution completes
  {
    const session = await pool().checkout()
    const snap = (await session.loadSnapshot(blob, { mount })) as FunctionSnapshot
    const done = (await snap.resume(null)) as MontyComplete
    t.is(done.output, 'hi')
    await session.close()
  }

  // omitted: validation rejects the load and poisons the session
  {
    const session = await pool().checkout()
    await t.throwsAsync(() => session.loadSnapshot(blob), { instanceOf: MontyRuntimeError })
    await t.throwsAsync(() => session.feedRun('1 + 1'))
    await session.close()
  }
})
