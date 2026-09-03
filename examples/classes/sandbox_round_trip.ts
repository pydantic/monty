// Sandbox instances re-enter the sandbox by identity.
//
// A `MontyClassProxy` carries the instance's `id`. Passing it back — as an
// input or an external-function result — hands the sandbox its ORIGINAL
// object, not a copy built from `attributes`; a proxy whose object the sandbox
// has freed raises.

import assert from 'node:assert/strict'
import { Monty, MontyClassProxy, MontyRuntimeError } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout()
await session.feedRun('class Counter:\n    def __init__(self):\n        self.n = 1\ncounter = Counter()')
const proxy = await session.feedRun('counter')
assert.ok(proxy instanceof MontyClassProxy)

// Back in as an input: same object, host-side edits to `attributes` are ignored.
proxy.attributes.n = 99
assert.equal(
  await session.feedRun('back is counter and back.n == 1', {
    inputs: { back: proxy },
  }),
  true,
)

// Back in as an external-function result.
const echo = (value: unknown) => value
assert.equal(
  await session.feedRun('echo(counter) is counter', {
    externalLookup: { echo },
  }),
  true,
)

// Once the sandbox drops its last reference (inputs persist as session
// globals, so `back` must go too), the proxy no longer resolves.
await session.feedRun('counter = back = None')
await assert.rejects(session.feedRun('back', { inputs: { back: proxy } }), (error: unknown) => {
  assert.ok(error instanceof MontyRuntimeError)
  console.log('freed object rejected:', error.message)
  return true
})

console.log(`proxy id ${proxy.id} resolved to the original sandbox object`)
