import { test } from 'vitest'
import { t } from './assertions.js'
import { skipIfNode } from './env.js'
import { setupPool } from './helpers.js'

const { pool } = setupPool()

test('browser uses a non-isolated Web Worker backend', (ctx) => {
  skipIfNode(ctx)
  t.false(globalThis.crossOriginIsolated)
})

test('browser wasm reports mounts as unsupported', async (ctx) => {
  skipIfNode(ctx)
  await using session = await pool().checkout()

  const error = await t.throwsAsync(() =>
    session.feedRun("open('/mnt/data/file.txt').read()", { mount: [{}] as never }),
  )
  t.is(error.message, 'the wasm worker does not support filesystem mounts (browser has no host filesystem)')
})
