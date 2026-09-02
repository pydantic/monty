// ClassType: let sandbox code instantiate a host class.
//
// `init: true` grants construction — the call runs host-side and the new
// instance crosses back governed by the `instance*` policies. Without
// `init: true` the sandbox gets `TypeError: cannot instantiate host class ...`.

import assert from 'node:assert/strict'
import { ClassType, Monty, MontyRuntimeError } from '@pydantic/monty'

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

await using pool = await Monty.create()
{
  await using session = await pool.checkout()
  const wrapper = new ClassType(Person, {
    init: true,
    instanceEagerAttrs: 'all',
    instanceAllowedMethods: 'all',
  })
  const result = await session.feedRun('p = Person("Samuel", 4)\np.greeting()', { inputs: { Person: wrapper } })
  assert.equal(result, 'hi Samuel')
  console.log('constructed in the sandbox:', result)
}
{
  await using session = await pool.checkout()
  try {
    await session.feedRun('Person("Samuel", 4)', {
      inputs: { Person: new ClassType(Person) },
    }) // init defaults to false
  } catch (error) {
    assert.ok(error instanceof MontyRuntimeError)
    console.log('construction denied:', error.message)
  }
}
