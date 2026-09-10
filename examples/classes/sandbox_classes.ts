// Classes defined inside Monty: instances reach the host as MontyClassProxy.
//
// Sandbox code can define its own classes (including dataclasses). An instance
// returned to the host is a read-only `MontyClassProxy` snapshot — `name`,
// `isDataclass`, `id`, and an `attributes` record — never live code.

import assert from 'node:assert/strict'
import { Monty, MontyClassProxy } from '@pydantic/monty'

const code = `\
from dataclasses import dataclass

@dataclass
class Point:
    x: int
    y: int

    def norm2(self) -> int:
        return self.x ** 2 + self.y ** 2

p = Point(3, 4)
assert p.norm2() == 25  # methods work inside the sandbox
p
`

await using pool = await Monty.create()
await using session = await pool.checkout()
const result = await session.feedRun(code)

assert.ok(result instanceof MontyClassProxy)
assert.equal(result.name, 'Point')
assert.equal(result.isDataclass, true)
assert.deepEqual({ ...result.attributes }, { x: 3, y: 4 })
console.log(`host received: ${result.name} ${JSON.stringify(result.attributes)}`)
