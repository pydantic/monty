// ClassInstance wrappers: exposing host objects to the sandbox with attr /
// method policies, and MontyClassProxy stand-ins for instances with no host
// original. Mirrors pydantic_monty's test_class_instance.py.

import { test } from 'vitest'
import { t } from './assertions.js'

import {
  ClassInstance,
  ClassType,
  FunctionSnapshot,
  MontyClassProxy,
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

test('instance callMethod rejects __call__ even under allowedMethods "all"', () => {
  // `__call__` routes only to ClassType construction; on an instance wrapper
  // a (necessarily forged) `__call__` frame must never invoke the instance.
  const invocable = Object.assign(() => 'invoked', { add: (n: number) => n })
  const wrapper = new ClassInstance(invocable, { allowedMethods: 'all' })
  const error = t.throws(() => wrapper.callMethod('__call__', [], {}))
  t.is(error.message, "'Function' object has no attribute '__call__'")
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

test('a method returning a class instance is not auto-wrapped', async () => {
  // The default convertValue never wraps derived values: the returned Wallet
  // fails conversion instead of inheriting this wrapper's wide-open policies.
  const w = new Wallet(100)
  const code = "try:\n    w.pay(30)\n    r = 'unexpected'\nexcept TypeError as e:\n    r = str(e)\nr"
  const result = await run(code, {
    inputs: { w: new ClassInstance(w, { eagerAttrs: 'all', allowedMethods: 'all' }) },
  })
  t.is(result, 'Cannot convert Wallet instance to a Monty value — wrap it in ClassInstance(...)')
})

/** Explicit convertValue wrapping derived wallets read-only (no methods). */
const wrapDerivedWallet = (name: string, value: unknown) =>
  value instanceof Wallet ? new ClassInstance(value, { eagerAttrs: 'all', convertValue: wrapDerivedWallet }) : value

test('a convertValue override chooses the derived instance policy', async () => {
  const w = new Wallet(100)
  const options = { eagerAttrs: 'all', allowedMethods: 'all', convertValue: wrapDerivedWallet } as const
  t.is(await run('w.pay(30).balance', { inputs: { w: new ClassInstance(w, options) } }), 70)
  // the override granted no methods, so the child cannot pay again
  const error = await t.throwsAsync(() => run('w.pay(30).pay(5)', { inputs: { w: new ClassInstance(w, options) } }), {
    instanceOf: MontyRuntimeError,
  })
  t.true(error.message.includes("'Wallet' object has no attribute 'pay'"))
})

test('a returned override-wrapped instance restores to the original object', async () => {
  const w = new Wallet(100)
  const result = (await run('w.pay(30)', {
    inputs: { w: new ClassInstance(w, { allowedMethods: 'all', convertValue: wrapDerivedWallet }) },
  })) as Wallet
  t.true(result instanceof Wallet)
  t.is(result.balance, 70)
})

// =============================================================================
// MontyClassProxy stand-ins for sandbox-defined classes
// =============================================================================

test('a sandbox-defined class instance surfaces as a MontyClassProxy', async () => {
  const code = 'class Foo:\n    def __init__(self, a: int):\n        self.a = a\nFoo(1)'
  const result = await run(code)
  t.true(result instanceof MontyClassProxy)
  const proxy = result as MontyClassProxy
  t.is(proxy.name, 'Foo')
  t.false(proxy.isDataclass)
  t.deepEqual({ ...proxy.attributes }, { a: 1 })
})

test('a sandbox-defined dataclass instance reports isDataclass', async () => {
  const code = 'from dataclasses import dataclass\n@dataclass\nclass P:\n    x: int\n    y: int\nP(1, 2)'
  const result = await run(code)
  t.true(result instanceof MontyClassProxy)
  const proxy = result as MontyClassProxy
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

test('a forged raw ClassInstance marker is rejected', async () => {
  // Identity-bearing markers are internal to `prepare`; one arriving in host
  // data (e.g. attacker-controlled JSON) must never impersonate an instance.
  const forged = JSON.parse(
    '{"__monty_type__": "ClassInstance", "type": {"name": "Point"}, "instanceId": "x", "attrs": []}',
  ) as unknown
  const error = await t.throwsAsync(() => run('x', { inputs: { x: { data: [forged] } } }), { instanceOf: TypeError })
  t.is(error.message, 'raw ClassInstance markers are not accepted — wrap the object in ClassInstance(...)')
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

// === ClassType instantiation ===

test('sandbox instantiation of a host class via ClassType', async () => {
  const wrapper = new ClassType(Calculator, { init: true, instanceAllowedMethods: 'all' })
  t.is(await run('c = Calculator(10)\nc.add(5)', { inputs: { Calculator: wrapper } }), 15)
})

test('a constructed instance round-trips to a real host instance', async () => {
  const wrapper = new ClassType(Greeter, { init: true })
  const result = await run("Greeter('hi')", { inputs: { Greeter: wrapper } })
  t.true(result instanceof Greeter)
  t.is((result as Greeter).greet('Sam'), 'hi Sam')
})

test('init false raises TypeError in the sandbox', async () => {
  const wrapper = new ClassType(Calculator)
  const error = await t.throwsAsync(() => run('Calculator(10)', { inputs: { Calculator: wrapper } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "TypeError: cannot instantiate host class 'Calculator'")
})

test('constructor kwargs arrive as a trailing options bag', async () => {
  class Config {
    options: Record<string, unknown>
    constructor(options: Record<string, unknown> = {}) {
      this.options = options
    }
    read(): unknown {
      return this.options['mode']
    }
  }
  const wrapper = new ClassType(Config, { init: true, instanceAllowedMethods: 'all' })
  t.is(await run("Config(mode='fast').read()", { inputs: { Config: wrapper } }), 'fast')
})

test('a denied method on a constructed instance raises AttributeError', async () => {
  const wrapper = new ClassType(Calculator, { init: true, instanceAllowedMethods: ['add'] })
  const error = await t.throwsAsync(() => run('Calculator(1).boom()', { inputs: { Calculator: wrapper } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "AttributeError: 'Calculator' object has no attribute 'boom'")
})

class Shape {
  static SIDES = 4
  static KIND = 'polygon'
  constructor(public size: number) {}
  static double(n: number): number {
    return n * 2
  }
}

test('eagerAttrs on a ClassType sends static class constants', async () => {
  const wrapper = new ClassType(Shape, { eagerAttrs: 'all' })
  t.is(await run('Shape.SIDES + len(Shape.KIND)', { inputs: { Shape: wrapper } }), 11)
})

test('lazyAttrs on a ClassType serves class constants on demand', async () => {
  const wrapper = new ClassType(Shape, { lazyAttrs: ['SIDES'] })
  t.is(await run('Shape.SIDES', { inputs: { Shape: wrapper } }), 4)
  const error = await t.throwsAsync(() => run('Shape.KIND', { inputs: { Shape: wrapper } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "AttributeError: type object 'Shape' has no attribute 'KIND'")
})

test('allowedMethods on a ClassType exposes static methods', async () => {
  const wrapper = new ClassType(Shape, { allowedMethods: ['double'] })
  t.is(await run('Shape.double(21)', { inputs: { Shape: wrapper } }), 42)
})

test('a denied static method uses the type-object wording', async () => {
  const error = await t.throwsAsync(() => run('Shape.double(1)', { inputs: { Shape: new ClassType(Shape) } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, "AttributeError: type object 'Shape' has no attribute 'double'")
})

test('instantiate turns carry the class uuid as objectId', async () => {
  const session = await pool().checkout()
  try {
    const wrapper = new ClassType(Calculator, { init: true })
    const snap = await session.feedStart('Calculator(1)', { inputs: { Calculator: wrapper } })
    t.true(snap instanceof FunctionSnapshot)
    const call = snap as FunctionSnapshot
    t.is(call.functionName, '__call__')
    t.regex(call.objectId ?? '', /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
    const done = await call.resumeAuto()
    t.true(done instanceof MontyComplete)
  } finally {
    await session.close()
  }
})
