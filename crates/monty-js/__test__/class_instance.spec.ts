// ClassInstance wrappers: exposing host objects to the sandbox with attr /
// method policies, and MontyClassInstance proxies for instances with no host
// original. Mirrors pydantic_monty's test_class_instance.py.

import { test } from 'vitest'
import { t } from './assertions.js'

import {
  ClassInstance,
  MontyClassInstance,
  MontyComplete,
  MontyRuntimeError,
  NameLookupSnapshot,
} from '@pydantic/monty'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

class Greeter {
  greeting: string
  _hidden = 'secret'

  constructor(greeting: string) {
    this.greeting = greeting
  }

  greet(name: string): string {
    return `${this.greeting} ${name}`
  }
}

class Calculator {
  value: number

  constructor(value: number) {
    this.value = value
  }

  add(n: number): number {
    return this.value + n
  }

  scale(kwargs: { factor?: number } = {}): number {
    return this.value * (kwargs.factor ?? 2)
  }

  combine(a: number, b: number, kwargs: { sep?: string } = {}): string {
    return [a, b].join(kwargs.sep ?? ',')
  }

  boom(): never {
    const error = new Error('nope')
    error.name = 'ValueError'
    throw error
  }

  async fetch(): Promise<number> {
    await new Promise((resolve) => setTimeout(resolve, 5))
    return this.value * 10
  }

  _secret(): number {
    return -1
  }
}

class Wallet {
  balance: number

  constructor(balance: number) {
    this.balance = balance
  }

  pay(amount: number): Wallet {
    return new Wallet(this.balance - amount)
  }
}

// =============================================================================
// Eager attrs
// =============================================================================

test('eagerAttrs all sends own non-underscore props', async () => {
  const g = new Greeter('hello')
  t.is(await run('x.greeting', { inputs: { x: new ClassInstance(g, { eagerAttrs: 'all' }) } }), 'hello')
})

test('eagerAttrs explicit list sends exactly those props', async () => {
  const c = new Calculator(5)
  t.is(await run('x.value + 1', { inputs: { x: new ClassInstance(c, { eagerAttrs: ['value'] }) } }), 6)
})

test('an attr outside the eager list raises AttributeError', async () => {
  const g = new Greeter('hello')
  const error = await t.throwsAsync(
    () => run('x.greeting', { inputs: { x: new ClassInstance(g, { eagerAttrs: [] }) } }),
    { instanceOf: MontyRuntimeError },
  )
  t.is(error.message, "AttributeError: 'Greeter' object has no attribute 'greeting'")
})

// =============================================================================
// Identity round-trip
// =============================================================================

test('returning a host-sent instance gives the original object back', async () => {
  const g = new Greeter('hello')
  t.is(await run('x', { inputs: { x: new ClassInstance(g, { eagerAttrs: 'all' }) } }), g)
})

test('the same instance round-trips across feeds in one session', async () => {
  const g = new Greeter('hello')
  const session = await pool().checkout()
  try {
    t.is(await session.feedRun('x', { inputs: { x: new ClassInstance(g, { eagerAttrs: 'all' }) } }), g)
    t.is(await session.feedRun('y', { inputs: { y: new ClassInstance(g, { eagerAttrs: 'all' }) } }), g)
  } finally {
    await session.close()
  }
})

// =============================================================================
// Method calls
// =============================================================================

test('sync method call with args', async () => {
  const c = new Calculator(5)
  t.is(await run('c.add(10)', { inputs: { c: new ClassInstance(c, { allowedMethods: ['add'] }) } }), 15)
})

test('method call allowed by "all"', async () => {
  const c = new Calculator(5)
  t.is(await run('c.add(10)', { inputs: { c: new ClassInstance(c, { allowedMethods: 'all' }) } }), 15)
})

test('kwargs are delivered as a trailing options bag', async () => {
  const c = new Calculator(5)
  t.is(await run('c.scale(factor=3)', { inputs: { c: new ClassInstance(c, { allowedMethods: 'all' }) } }), 15)
})

test('method call with args and kwargs', async () => {
  const c = new Calculator(5)
  const result = await run("c.combine(1, 2, sep='-')", {
    inputs: { c: new ClassInstance(c, { allowedMethods: 'all' }) },
  })
  t.is(result, '1-2')
})

test('promise-returning method resolves via the future machinery', async () => {
  const c = new Calculator(4)
  t.is(await run('await c.fetch()', { inputs: { c: new ClassInstance(c, { allowedMethods: 'all' }) } }), 40)
})

test('denied method raises AttributeError', async () => {
  const c = new Calculator(5)
  const error = await t.throwsAsync(
    () => run('c.add(1)', { inputs: { c: new ClassInstance(c, { allowedMethods: ['scale'] }) } }),
    { instanceOf: MontyRuntimeError },
  )
  t.is(error.message, "AttributeError: 'Calculator' object has no attribute 'add'")
})

test('no allowedMethods policy denies every method', async () => {
  const c = new Calculator(5)
  const error = await t.throwsAsync(() => run('c.add(1)', { inputs: { c: new ClassInstance(c) } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "AttributeError: 'Calculator' object has no attribute 'add'")
})

test('a throwing method surfaces its error in the sandbox', async () => {
  const c = new Calculator(5)
  const code = "try:\n    c.boom()\n    r = 'unexpected'\nexcept ValueError as e:\n    r = str(e)\nr"
  t.is(await run(code, { inputs: { c: new ClassInstance(c, { allowedMethods: 'all' }) } }), 'nope')
})

test('underscore methods are never dispatched, even with "all"', async () => {
  const c = new Calculator(5)
  const error = await t.throwsAsync(
    () => run('c._secret()', { inputs: { c: new ClassInstance(c, { allowedMethods: 'all' }) } }),
    { instanceOf: MontyRuntimeError },
  )
  t.is(error.message, "AttributeError: 'Calculator' object has no attribute '_secret'")
})

// =============================================================================
// Lazy attrs
// =============================================================================

test('lazy attr allowed by an explicit set', async () => {
  const g = new Greeter('hello')
  t.is(await run('g.greeting', { inputs: { g: new ClassInstance(g, { lazyAttrs: new Set(['greeting']) }) } }), 'hello')
})

test('lazy attr allowed by "all"', async () => {
  const g = new Greeter('hello')
  t.is(await run('g.greeting', { inputs: { g: new ClassInstance(g, { lazyAttrs: 'all' }) } }), 'hello')
})

test('lazy attr outside the policy raises AttributeError', async () => {
  const g = new Greeter('hello')
  const error = await t.throwsAsync(
    () => run('g.greeting', { inputs: { g: new ClassInstance(g, { lazyAttrs: ['other'] }) } }),
    { instanceOf: MontyRuntimeError },
  )
  t.is(error.message, "AttributeError: 'Greeter' object has no attribute 'greeting'")
})

test('underscore attrs never reach the host', async () => {
  const g = new Greeter('hello')
  const convertCalls: string[] = []
  const wrapper = new ClassInstance(g, {
    lazyAttrs: 'all',
    convertValue: (name, value) => {
      convertCalls.push(name)
      return value
    },
  })
  const error = await t.throwsAsync(() => run('g._hidden', { inputs: { g: wrapper } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "AttributeError: 'Greeter' object has no attribute '_hidden'")
  // the sandbox blocks the lookup before it suspends: the wrapper is never consulted
  t.deepEqual(convertCalls, [])
})

// =============================================================================
// convertValue and child wrappers
// =============================================================================

test('convertValue transforms eager attrs and method returns', async () => {
  const g = new Greeter('hello')
  const upper = (name: string, value: unknown) => (typeof value === 'string' ? value.toUpperCase() : value)
  t.is(
    await run('g.greeting', { inputs: { g: new ClassInstance(g, { eagerAttrs: 'all', convertValue: upper }) } }),
    'HELLO',
  )
  t.is(
    await run("g.greet('sam')", {
      inputs: { g: new ClassInstance(g, { allowedMethods: 'all', convertValue: upper }) },
    }),
    'HELLO SAM',
  )
})

test('a method returning a class instance is auto-wrapped with the same policies', async () => {
  const w = new Wallet(100)
  const result = await run('w.pay(30).balance', {
    inputs: { w: new ClassInstance(w, { eagerAttrs: 'all', allowedMethods: 'all' }) },
  })
  t.is(result, 70)
})

test('a returned auto-wrapped instance restores to the original object', async () => {
  const w = new Wallet(100)
  const result = (await run('w.pay(30)', {
    inputs: { w: new ClassInstance(w, { eagerAttrs: 'all', allowedMethods: 'all' }) },
  })) as Wallet
  t.true(result instanceof Wallet)
  t.is(result.balance, 70)
})

// =============================================================================
// MontyClassInstance proxies for sandbox-defined classes
// =============================================================================

test('a sandbox-defined class instance surfaces as a MontyClassInstance', async () => {
  const code = 'class Foo:\n    def __init__(self, a: int):\n        self.a = a\nFoo(1)'
  const result = await run(code)
  t.true(result instanceof MontyClassInstance)
  const proxy = result as MontyClassInstance
  t.is(proxy.name, 'Foo')
  t.false(proxy.isDataclass)
  t.deepEqual({ ...proxy.attributes }, { a: 1 })
})

test('a sandbox-defined dataclass instance reports isDataclass', async () => {
  const code = 'from dataclasses import dataclass\n@dataclass\nclass P:\n    x: int\n    y: int\nP(1, 2)'
  const result = await run(code)
  t.true(result instanceof MontyClassInstance)
  const proxy = result as MontyClassInstance
  t.is(proxy.name, 'P')
  t.true(proxy.isDataclass)
  t.deepEqual({ ...proxy.attributes }, { x: 1, y: 2 })
})

// =============================================================================
// Non-plain object rejection
// =============================================================================

test('an unwrapped class instance input is rejected with a wrap hint', async () => {
  const error = await t.throwsAsync(() => run('x', { inputs: { x: new Greeter('hi') } }), { instanceOf: TypeError })
  t.is(error.message, 'Cannot convert Greeter instance to a Monty value — wrap it in ClassInstance(...)')
})

test('an unwrapped instance returned from an external function raises in the sandbox', async () => {
  const code = "try:\n    bad()\n    r = 'unexpected'\nexcept TypeError as e:\n    r = str(e)\nr"
  const result = await run(code, { externalLookup: { bad: () => new Greeter('hi') } })
  t.is(result, 'Cannot convert Greeter instance to a Monty value — wrap it in ClassInstance(...)')
})

// =============================================================================
// PR-review fixes: depth cap, realm-safe policies, snapshot resumeValue
// =============================================================================

test('a too-deep input fails with a conversion error, not a stack overflow', async () => {
  let nested: unknown = 1
  for (let i = 0; i < 60; i++) {
    nested = [nested]
  }
  const error = await t.throwsAsync(() => run('x', { inputs: { x: nested } }), { instanceOf: TypeError })
  t.is(error.message, 'Max input depth exceeded')
})

test('a set-like policy from another realm works (duck-typed .has)', async () => {
  // simulate a cross-realm ReadonlySet: conforms to the interface but fails
  // `instanceof Set` in this realm
  const setLike = { has: (name: string) => name === 'greet' } as unknown as ReadonlySet<string>
  const wrapper = new ClassInstance(new Greeter('hi'), { allowedMethods: setLike })
  t.is(await run("g.greet('Sam')", { inputs: { g: wrapper } }), 'hi Sam')
})

test('NameLookupSnapshot.resumeValue answers a lazy attribute lookup by hand', async () => {
  const session = await pool().checkout()
  try {
    const wrapper = new ClassInstance(new Greeter('hi'), { lazyAttrs: 'all' })
    const snap = await session.feedStart('g.greeting + "!"', { inputs: { g: wrapper } })
    t.true(snap instanceof NameLookupSnapshot)
    const lookup = snap as NameLookupSnapshot
    t.is(lookup.variableName, 'greeting')
    t.not(lookup.instanceId, null)
    const done = await lookup.resumeValue('manual')
    t.true(done instanceof MontyComplete)
    t.is((done as MontyComplete).output, 'manual!')
  } finally {
    await session.close()
  }
})
