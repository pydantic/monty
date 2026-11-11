// A closed session must be garbage-collectable: native callbacks the session
// hands its checkout hold their JS functions as GC roots, so any that outlive
// `close()` keep the session and everything it references alive.

import { setImmediate } from 'node:timers/promises'
import { setFlagsFromString } from 'node:v8'
import { runInNewContext } from 'node:vm'

import { test } from 'vitest'
import { t } from './assertions.js'

import type { FeedOptions } from '@pydantic/monty'
import { setupPool } from './helpers.js'

setFlagsFromString('--expose-gc')
const gc = runInNewContext('gc') as () => void

const { pool } = setupPool({ streamAsyncCalls: true })

/** Module-scoped: a registry that is itself collected never runs its callbacks. */
const collectedSessions = new Set<number>()
const registry = new FinalizationRegistry((id: number) => collectedSessions.add(id))
let lastSessionId = 0

/** Runs `feeds` in a fresh session and closes it, returning only whether it has been collected. */
async function closedSession(feeds: [string, FeedOptions][]): Promise<() => boolean> {
  const id = ++lastSessionId
  const session = await pool().checkout()
  for (const [code, options] of feeds) {
    await session.feedRun(code, options)
  }
  await session.close()
  registry.register(session, id)
  return () => collectedSessions.has(id)
}

/** Forces collections until `collected()` holds, or gives up after a bounded number of rounds. */
async function collect(collected: () => boolean): Promise<boolean> {
  for (let round = 0; round < 20 && !collected(); round++) {
    gc()
    // finalizers run as their own task, after the collection
    await setImmediate()
  }
  return collected()
}

test('a closed session that streamed calls is collected', async () => {
  const fetch = async (x: number) => x + 1
  const collected = await closedSession([
    ['import asyncio\nawait asyncio.gather(fetch(1), fetch(2))', { externalLookup: { fetch } }],
  ])
  t.true(await collect(collected))
})

test('a closed session that never streamed is collected', async () => {
  const collected = await closedSession([['1 + 1', {}]])
  t.true(await collect(collected))
})
