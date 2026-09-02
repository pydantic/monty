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

/** A host object whose lazy attributes fail on the host side. */
class Flaky {
  value = 1

  get boom(): never {
    const error = new Error('boom')
    error.name = 'KeyError'
    throw error
  }

  get raw(): Greeter {
    return new Greeter('hi')
  }

  get sym(): symbol {
    return Symbol('nope')
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

test('getattr/hasattr consult lazy attrs like g.attr', async () => {
  const g = new Greeter('hello')
  const inputs = { g: new ClassInstance(g, { lazyAttrs: new Set(['greeting']) }) }
  const code = "[hasattr(g, 'greeting'), getattr(g, 'greeting'), hasattr(g, 'other'), getattr(g, 'other', 7)]"
  t.deepEqual(await run(code, { inputs }), [true, 'hello', false, 7])
  const error = await t.throwsAsync(() => run("getattr(g, 'other')", { inputs }), { instanceOf: MontyRuntimeError })
  t.is(error.message, "AttributeError: 'Greeter' object has no attribute 'other'")
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

const CATCH_BOOM = "try:\n    f.boom\n    r = 'unexpected'\nexcept KeyError as e:\n    r = str(e)\nr"

test('a throwing lazy getter raises its error in the sandbox', async () => {
  t.is(await run(CATCH_BOOM, { inputs: { f: new ClassInstance(new Flaky(), { lazyAttrs: 'all' }) } }), "'boom'")
})

test('hasattr and getattr defaults do not swallow a lazy attribute host error', async () => {
  // only AttributeError is swallowed, as in CPython
  const inputs = { f: new ClassInstance(new Flaky(), { lazyAttrs: 'all' }) }
  for (const code of ["hasattr(f, 'boom')", "getattr(f, 'boom', 7)"]) {
    const error = await t.throwsAsync(() => run(code, { inputs }), { instanceOf: MontyRuntimeError })
    t.is(error.message, 'KeyError: boom')
  }
})

test('a convertValue throw on a lazy attr raises in the sandbox', async () => {
  const wrapper = new ClassInstance(new Greeter('hi'), {
    lazyAttrs: 'all',
    convertValue: (name) => {
      const error = new Error(`cannot convert ${name}`)
      error.name = 'ValueError'
      throw error
    },
  })
  const code = "try:\n    g.greeting\n    r = 'unexpected'\nexcept ValueError as e:\n    r = str(e)\nr"
  t.is(await run(code, { inputs: { g: wrapper } }), 'cannot convert greeting')
})

test('an unconvertible lazy value raises TypeError in the sandbox', async () => {
  // `raw` fails in `prepare` (an unwrapped instance), `sym` in the native
  // conversion; both reach sandbox code, unlike a plain externalLookup value
  const inputs = { f: new ClassInstance(new Flaky(), { lazyAttrs: 'all' }) }
  const catching = (attr: string) =>
    `try:\n    f.${attr}\n    r = 'unexpected'\nexcept TypeError as e:\n    r = str(e)\nr`
  t.is(
    await run(catching('raw'), { inputs }),
    'Cannot convert Greeter instance to a Monty value — wrap it in ClassInstance(...)',
  )
  t.is(await run(catching('sym'), { inputs }), 'Cannot convert JS Symbol to Monty value')
})

test('the session stays usable after a lazy attribute host error', async () => {
  const session = await pool().checkout()
  try {
    const inputs = { f: new ClassInstance(new Flaky(), { lazyAttrs: 'all' }) }
    const error = await t.throwsAsync(() => session.feedRun('f.boom', { inputs }), { instanceOf: MontyRuntimeError })
    t.is(error.message, 'KeyError: boom')
    t.is(await session.feedRun('f.value + 1', { inputs }), 2)
  } finally {
    await session.close()
  }
})

test('NameLookupSnapshot.resumeAuto raises a lazy attribute host error in the sandbox', async () => {
  const session = await pool().checkout()
  try {
    const snap = await session.feedStart(CATCH_BOOM, {
      inputs: { f: new ClassInstance(new Flaky(), { lazyAttrs: 'all' }) },
    })
    t.true(snap instanceof NameLookupSnapshot)
    t.is((snap as NameLookupSnapshot).variableName, 'boom')
    const done = await (snap as NameLookupSnapshot).resumeAuto()
    t.true(done instanceof MontyComplete)
    t.is((done as MontyComplete).output, "'boom'")
  } finally {
    await session.close()
  }
})

// =============================================================================
// convertValue and child wrappers
// =============================================================================

test('convertValue transforms eager attrs and method returns', async () => {
  const g = new Greeter('hello')
  const upper = (_name: string, value: unknown) => (typeof value === 'string' ? value.toUpperCase() : value)
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
const wrapDerivedWallet = (_name: string, value: unknown) =>
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

test('a MontyClassProxy passed back hands the sandbox its original object', async () => {
  const session = await pool().checkout()
  try {
    await session.feedRun('class Foo:\n    def __init__(self):\n        self.x = 1\nfoo = Foo()')
    const proxy = await session.feedRun('foo')
    t.true(proxy instanceof MontyClassProxy)
    t.is(typeof (proxy as MontyClassProxy).id, 'string')
    t.true(await session.feedRun('back is foo and isinstance(back, Foo) and back.x == 1', { inputs: { back: proxy } }))
    t.true(await session.feedRun('echo(foo) is foo', { externalLookup: { echo: (value: unknown) => value } }))
  } finally {
    await session.close()
  }
})

test('a MontyClassProxy of a freed sandbox object is rejected', async () => {
  const session = await pool().checkout()
  try {
    const proxy = (await session.feedRun('class Foo:\n    pass\nFoo()')) as MontyClassProxy
    t.true(proxy instanceof MontyClassProxy)
    const error = await t.throwsAsync(() => session.feedRun('x', { inputs: { x: proxy } }), {
      instanceOf: MontyRuntimeError,
    })
    t.is(
      error.message.replace(proxy.id, '<id>'),
      "RuntimeError: invalid input type: sandbox instance of 'Foo' (id <id>) no longer exists",
    )
  } finally {
    await session.close()
  }
})

test('a MontyClassProxy round-trips nested in containers and its edited attributes are ignored', async () => {
  const session = await pool().checkout()
  try {
    await session.feedRun('class Foo:\n    def __init__(self):\n        self.x = 1\nfoo = Foo()')
    const proxy = (await session.feedRun('foo')) as MontyClassProxy
    proxy.attributes.x = 99
    const inputs = { items: [proxy], mapping: new Map([['k', proxy]]) }
    t.true(await session.feedRun("items[0] is foo and mapping['k'] is foo and foo.x == 1", { inputs }))
  } finally {
    await session.close()
  }
})

test('a MontyClassProxy still resolves after dump and loadSession', async () => {
  let blob: Buffer
  let proxy: MontyClassProxy
  {
    const session = await pool().checkout()
    await session.feedRun('class Foo:\n    def __init__(self):\n        self.x = 1\nfoo = Foo()')
    proxy = (await session.feedRun('foo')) as MontyClassProxy
    blob = await session.dump()
    await session.close()
  }
  const session = await pool().checkout()
  try {
    await session.loadSession(blob)
    t.true(await session.feedRun('back is foo and isinstance(back, Foo)', { inputs: { back: proxy } }))
  } finally {
    await session.close()
  }
})

test('a host-origin proxy from a restored session re-enters as a host-backed copy', async () => {
  let blob: Buffer
  {
    const session = await pool().checkout()
    await session.feedRun('x = obj', {
      inputs: { obj: new ClassInstance(new Greeter('hello'), { eagerAttrs: 'all' }) },
    })
    blob = await session.dump()
    await session.close()
  }
  const session = await pool().checkout()
  try {
    await session.loadSession(blob)
    const proxy = await session.feedRun('x')
    t.true(proxy instanceof MontyClassProxy)
    t.deepEqual(await session.feedRun('[type(y).__name__, y.greeting, y is x, y == x]', { inputs: { y: proxy } }), [
      'Greeter',
      'hello',
      false,
      true,
    ])
  } finally {
    await session.close()
  }
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

test('a duplicate wrapper id wrapping a different object is rejected', async () => {
  // Silent overwrite would re-route values the sandbox already holds from
  // one host object to the other.
  const id = '11111111-2222-4333-8444-555555555555'
  const a = new ClassInstance(new Greeter('hi'), { id })
  const b = new ClassInstance(new Greeter('yo'), { id })
  const error = await t.throwsAsync(() => run('[a, b]', { inputs: { a, b } }), { instanceOf: TypeError })
  t.is(error.message, `wrapper id '${id}' already identifies a different object in this session`)
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
    t.not(lookup.objectId, null)
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

test('a constructed instance keeps the ClassType id and name', async () => {
  const wrapper = new ClassType(Wallet, {
    id: '12345678-1234-4123-8123-123456789abc',
    name: 'Purse',
    init: true,
    instanceEagerAttrs: 'all',
  })
  const code = 'w = Wallet(1)\n[type(w) == Wallet, type(w) is Wallet, type(w).__name__, isinstance(w, Wallet)]'
  t.deepEqual(await run(code, { inputs: { Wallet: wrapper } }), [true, true, 'Purse', true])
})

test('a constructed instance of another class gets a default ClassType', () => {
  class Other {
    v = 1
  }
  class Factory {
    constructor() {
      return new Other()
    }
  }
  const wrapper = new ClassType(Factory, { init: true })
  const wrapped = wrapper.construct([], {})
  t.true(wrapped.classType !== wrapper)
  t.is(wrapped.classType.classType, Other as never)
})

test('an own constructor property does not change class identity', async () => {
  class Other {}
  class Shadowing {
    n = 1
  }
  const instance = new Shadowing()
  Object.defineProperty(instance, 'constructor', { value: Other })
  const wrapper = new ClassType(Shadowing, { init: true, instanceEagerAttrs: 'all' })
  t.is(wrapper.instanceWrapper(instance).classType, wrapper)
  t.is(new ClassInstance(instance).getName(), 'Shadowing')
  t.is(new ClassInstance(instance, { classType: wrapper }).classType, wrapper)
  const inputs = { x: wrapper.instanceWrapper(instance), Shadowing: wrapper }
  t.deepEqual(await run('[type(x).__name__, isinstance(x, Shadowing), x.n]', { inputs }), ['Shadowing', true, 1])
})

test("an instance's type branch carries the ClassType's eager attrs", async () => {
  const classType = new ClassType(Shape, { eagerAttrs: ['SIDES'] })
  const inputs = { x: new ClassInstance(new Shape(1), { classType }) }
  t.deepEqual(await run('[type(x).SIDES, x.__class__.SIDES]', { inputs }), [4, 4])
})

test('type objects of two instances are one object', async () => {
  const inputs = { a: new ClassInstance(new Shape(1)), b: new ClassInstance(new Shape(2)) }
  t.deepEqual(await run('[type(a) is type(b), {type(a): 1}[type(b)], isinstance(a, type(b))]', { inputs }), [
    true,
    1,
    true,
  ])
})

test('a ClassType name override reaches the instance and its error message', async () => {
  const classType = new ClassType(Shape, { name: 'Polygon' })
  const inputs = { x: new ClassInstance(new Shape(1), { classType }) }
  t.is(await run('type(x).__name__', { inputs }), 'Polygon')
  const error = await t.throwsAsync(() => run('x.missing', { inputs }), { instanceOf: MontyRuntimeError })
  t.is(error.message, "AttributeError: 'Polygon' object has no attribute 'missing'")
})

test('name on a ClassInstance names its default ClassType, and clashes with classType', async () => {
  t.is(await run('type(x).__name__', { inputs: { x: new ClassInstance(new Shape(1), { name: 'Poly' }) } }), 'Poly')
  t.throws(() => new ClassInstance(new Shape(1), { name: 'Poly', classType: new ClassType(Shape) }), {
    instanceOf: TypeError,
    message: 'pass name on the ClassType wrapper, not alongside classType',
  })
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

// =============================================================================
// Policy hardening: prototype built-ins, 'all' scope, id normalisation
// =============================================================================

/** The `AttributeError` message `f()` raises in the sandbox, for probing denied names. */
const PROBE =
  'def probe(f):\n    try:\n        f()\n    except AttributeError as e:\n        return str(e)\n    return None\n'

test('JS object machinery is denied on an instance under "all", and the host object is untouched', async () => {
  const c = new Calculator(5)
  const inputs = { c: new ClassInstance(c, { allowedMethods: 'all', lazyAttrs: 'all' }) }
  const code = [
    'probe(lambda: c.constructor(99))',
    'probe(lambda: c.toString())',
    'probe(lambda: c.call(None, 1))',
    "probe(lambda: c.hasOwnProperty('value'))",
    'probe(lambda: c.arguments)',
    "hasattr(c, 'constructor')",
    'c.add(1)',
  ]
  t.deepEqual(await run(`${PROBE}[${code.join(', ')}]`, { inputs }), [
    "'Calculator' object has no attribute 'constructor'",
    "'Calculator' object has no attribute 'toString'",
    "'Calculator' object has no attribute 'call'",
    "'Calculator' object has no attribute 'hasOwnProperty'",
    "'Calculator' object has no attribute 'arguments'",
    false,
    6,
  ])
  t.is(c.value, 5)
  t.is(Object.getPrototypeOf(c), Calculator.prototype)
})

test('JS function machinery is denied on a class under "all"', async () => {
  const inputs = { Shape: new ClassType(Shape, { allowedMethods: 'all', lazyAttrs: 'all' }) }
  const code = [
    'probe(lambda: Shape.constructor("return 1"))',
    'probe(lambda: Shape.call(None, 1))',
    'probe(lambda: Shape.toString())',
    'probe(lambda: Shape.prototype)',
    'probe(lambda: Shape.arguments)',
    'Shape.double(21)',
    'Shape.SIDES',
  ]
  t.deepEqual(await run(`${PROBE}[${code.join(', ')}]`, { inputs }), [
    "type object 'Shape' has no attribute 'constructor'",
    "type object 'Shape' has no attribute 'call'",
    "type object 'Shape' has no attribute 'toString'",
    "type object 'Shape' has no attribute 'prototype'",
    "type object 'Shape' has no attribute 'arguments'",
    42,
    4,
  ])
  t.is(Shape.SIDES, 4)
})

test('an explicit policy cannot name JS object machinery either', () => {
  const wrapper = new ClassInstance(new Calculator(5), {
    allowedMethods: ['constructor', 'add'],
    lazyAttrs: ['__proto__', 'prototype', 'caller', 'value'],
  })
  t.is(wrapper.callMethod('add', [1], {}), 6)
  t.is(wrapper.lookupLazyAttr('value'), 5)
  for (const name of ['constructor', '__proto__', 'prototype', 'caller']) {
    t.is(t.throws(() => wrapper.callMethod(name, [], {})).message, `'Calculator' object has no attribute '${name}'`)
    t.is(t.throws(() => wrapper.lookupLazyAttr(name)).message, `'Calculator' object has no attribute '${name}'`)
  }
})

test('a wrapped function cannot be invoked through Function.prototype', () => {
  const tool = Object.assign((x: number) => x * 2, { double: (x: number) => x * 2 })
  const wrapper = new ClassInstance(tool, { allowedMethods: 'all', lazyAttrs: 'all' })
  for (const name of ['call', 'apply', 'bind']) {
    t.is(
      t.throws(() => wrapper.callMethod(name, [null, 21], {})).message,
      `'Function' object has no attribute '${name}'`,
    )
  }
  // own function props are ordinary host data
  t.is(wrapper.lookupLazyAttr('length'), 1)
})

test('"all" exposes methods the class defines, not callables stored on the instance', async () => {
  class Bag {
    run = () => 'ran'
    static Inner = class {}
    helper(): string {
      return 'helped'
    }
    static make(): string {
      return 'made'
    }
  }
  const bag = new Bag()
  const all = {
    b: new ClassInstance(bag, { allowedMethods: 'all' }),
    Bag: new ClassType(Bag, { allowedMethods: 'all' }),
  }
  const code = ['b.helper()', 'probe(lambda: b.run())', 'Bag.make()', 'probe(lambda: Bag.Inner())']
  t.deepEqual(await run(`${PROBE}[${code.join(', ')}]`, { inputs: all }), [
    'helped',
    "'Bag' object has no attribute 'run'",
    'made',
    "type object 'Bag' has no attribute 'Inner'",
  ])
  // an explicit list names whatever the host chose, stored callables included
  t.is(await run('b.run()', { inputs: { b: new ClassInstance(bag, { allowedMethods: ['run'] }) } }), 'ran')
})

test('a string policy other than "all" is rejected at construction', () => {
  const g = new Greeter('hi')
  t.throws(() => new ClassInstance(g, { allowedMethods: 'greet' as never }), {
    instanceOf: TypeError,
    message: "allowedMethods must be 'all', undefined or a list/Set of names, got 'greet'",
  })
  t.throws(() => new ClassInstance(g, { eagerAttrs: 'greeting' as never }), {
    instanceOf: TypeError,
    message: "eagerAttrs must be 'all', undefined or a list/Set of names, got 'greeting'",
  })
  t.throws(() => new ClassType(Greeter, { instanceLazyAttrs: 'greeting' as never }), {
    instanceOf: TypeError,
    message: "instanceLazyAttrs must be 'all', undefined or a list/Set of names, got 'greeting'",
  })
})

test('a __proto__ keyword argument never reaches the host', async () => {
  const c = new Calculator(5)
  const inputs = { c: new ClassInstance(c, { allowedMethods: 'all' }) }
  // the only kwarg is dropped, so no options bag is appended and `sep` keeps its default
  t.is(await run("c.combine(1, 2, __proto__={'sep': '-'})", { inputs }), '1,2')
  t.is(Object.getPrototypeOf(c), Calculator.prototype)
})

test('an explicit id is validated and lowercased, and still round-trips to the original', async () => {
  const g = new Greeter('hi')
  const wrapper = new ClassInstance(g, { id: 'ABCDEF01-2345-4678-89AB-CDEF01234567' })
  t.is(wrapper.id, 'abcdef01-2345-4678-89ab-cdef01234567')
  t.is(await run('x', { inputs: { x: wrapper } }), g)
  const classType = new ClassType(Shape, { id: '12345678-1234-4123-8123-123456789ABC' })
  t.is(classType.id, '12345678-1234-4123-8123-123456789abc')
  t.is(await run('Shape', { inputs: { Shape: classType } }), Shape)
  t.throws(() => new ClassInstance(g, { id: 'not-a-uuid' }), {
    instanceOf: TypeError,
    message: 'ClassInstance id must be a canonical uuid string, got "not-a-uuid"',
  })
  t.throws(() => new ClassType(Shape, { id: '12345678123441238123123456789abc' }), {
    instanceOf: TypeError,
    message: 'ClassType id must be a canonical uuid string, got "12345678123441238123123456789abc"',
  })
})

test('a returned host class resolves to the class object when registered', async () => {
  t.is(await run('Shape', { inputs: { Shape: new ClassType(Shape) } }), Shape)
  t.is(await run('type(x)', { inputs: { x: new ClassInstance(new Shape(1)) } }), Shape)
  t.deepEqual(await run('[type(x), [x.__class__]]', { inputs: { x: new ClassInstance(new Shape(1)) } }), [
    Shape,
    [Shape],
  ])
})

test('a returned host class stays a Type marker when the session never registered it', async () => {
  let blob: Buffer
  {
    const session = await pool().checkout()
    await session.feedRun('x = obj', { inputs: { obj: new ClassInstance(new Greeter('hello')) } })
    blob = await session.dump()
    await session.close()
  }
  const session = await pool().checkout()
  try {
    await session.loadSession(blob)
    const marker = (await session.feedRun('type(x)')) as { __monty_type__: string; classType: { name: string } }
    t.is(marker.__monty_type__, 'Type')
    t.is(marker.classType.name, 'Greeter')
  } finally {
    await session.close()
  }
})

test('a forged host-class Type marker is rejected, builtin type markers still pass prepare', async () => {
  const forged = JSON.parse(
    '{"__monty_type__": "Type", "classType": {"name": "Shape", "id": "12345678-1234-4123-8123-123456789abc"}}',
  ) as unknown
  const error = await t.throwsAsync(() => run('x', { inputs: { x: [forged] } }), { instanceOf: TypeError })
  t.is(error.message, 'raw Type markers are not accepted — pass the class through ClassType(...)')
  // a builtin type marker carries no identity: `prepare` passes it through and
  // the native layer decides (it cannot be an input, but that is not a forgery)
  const native = await t.throwsAsync(() => run('x', { inputs: { x: { __monty_type__: 'Type', value: 'int' } } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(native.message, "RuntimeError: invalid input type: 'Repr' is not a valid input value")
})
