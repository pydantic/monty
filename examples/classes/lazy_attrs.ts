// ClassInstance lazy attributes: fetched from the host on demand.
//
// `lazyAttrs` names cross only when sandbox code reads them — each access
// suspends the sandbox and asks the host. Anything outside the policy raises
// the usual `AttributeError` inside the sandbox.

import assert from 'node:assert/strict'
import { ClassInstance, Monty, MontyRuntimeError } from '@pydantic/monty'

class Config {
  retries = 3
  apiKey = 'hunter2' // never exposed below
}

await using pool = await Monty.create()
await using session = await pool.checkout()
const wrapper = new ClassInstance(new Config(), { lazyAttrs: ['retries'] })
assert.equal(await session.feedRun('cfg.retries', { inputs: { cfg: wrapper } }), 3)

try {
  await session.feedRun('cfg.apiKey', { inputs: { cfg: wrapper } })
} catch (error) {
  assert.ok(error instanceof MontyRuntimeError)
  console.log('denied as expected:', error.message)
}
