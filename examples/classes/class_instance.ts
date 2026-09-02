// ClassInstance: expose a host object to the sandbox with an explicit policy.
//
// `eagerAttrs` sends attribute values with the object, `allowedMethods` lets the
// sandbox call back into the real instance, and returning the object from
// sandbox code hands the host back the ORIGINAL object, not a copy.

import assert from 'node:assert/strict'
import { ClassInstance, Monty } from '@pydantic/monty'

class Person {
  name: string
  age: number
  constructor(name: string, age: number) {
    this.name = name
    this.age = age
  }
  greeting(): string {
    return `hi ${this.name}`
  }
}

const person = new Person('Samuel', 4)

await using pool = await Monty.create()
await using session = await pool.checkout()
const result = await session.feedRun('assert user.name == "Samuel"\nassert user.greeting() == "hi Samuel"\nuser', {
  inputs: {
    user: new ClassInstance(person, {
      eagerAttrs: 'all',
      allowedMethods: ['greeting'],
    }),
  },
})

assert.equal(result, person) // identity round-trip
console.log('got back the original object:', result)
