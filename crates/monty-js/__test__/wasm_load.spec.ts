// The turnkey loader: `createMonty()` auto-loads the bundled wasm (no caller
// module) and returns a ready pool. Exercises the `node` export entry's loader
// against the `.wasm` copied next to it by scripts/copy-wasm.mjs.
//
// Requires the release wasm build + copy:
//   cd crates/monty-js && npm run build:wasm

import test from 'ava'

import { createMonty } from '../ts/worker/index.node.js'
import { createMonty as createMontyDefault } from '../ts/worker/index.js'

test('createMonty auto-loads the wasm and runs a feed', async (t) => {
  const pool = await createMonty()
  const s = await pool.checkout()
  t.is(await s.feedRun('21 * 2'), 42)
  await s.feedRun('x = 5')
  t.is(await s.feedRun('x + 1'), 6)
  await s.close()
  await pool.close()
})

test('the default entry has no auto-loader and points to createWorkerPool', async (t) => {
  await t.throwsAsync(() => createMontyDefault(), {
    message: /could not auto-load .* createWorkerPool/,
  })
})
