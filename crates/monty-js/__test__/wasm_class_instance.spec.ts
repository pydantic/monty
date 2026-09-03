// The wasm path converts class-instance and class-type arena nodes in
// TypeScript (`ts/worker/value.ts`) rather than through napi; these
// round-trips cover every node kind a host class crossing produces.

import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'
import { ClassInstance, ClassType, Monty, MontyClassProxy, MontyRuntimeError } from '@pydantic/monty/wasm'

/** A host object whose lazy attribute fails on the host side. */
class Flaky {
  value = 1

  get boom(): never {
    const error = new Error('boom')
    error.name = 'KeyError'
    throw error
  }

  get sym(): symbol {
    return Symbol('nope')
  }
}

class Point {
  static DIMS = 2
  constructor(
    public x: number,
    public y: number,
  ) {}
  sum(): number {
    return this.x + this.y
  }
  static describe(): string {
    return 'a point'
  }
}

test('a ClassInstance round-trips over the wasm transport', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create()
  await using session = await pool.checkout({})

  const point = new Point(1, 2)
  const wrapper = new ClassInstance(point, { eagerAttrs: 'all', allowedMethods: 'all' })
  t.deepEqual(await session.feedRun('[p.x, p.y, p.sum(), type(p).__name__]', { inputs: { p: wrapper } }), [
    1,
    2,
    3,
    'Point',
  ])
  // identity is preserved, nested in a container too
  t.is(await session.feedRun('p', { inputs: { p: wrapper } }), point)
  t.deepEqual(await session.feedRun('[p, [p]]', { inputs: { p: wrapper } }), [point, [point]])
})

test('a ClassType round-trips over the wasm transport', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create()
  await using session = await pool.checkout({})

  const wrapper = new ClassType(Point, {
    init: true,
    eagerAttrs: 'all',
    allowedMethods: 'all',
    instanceEagerAttrs: 'all',
  })
  t.deepEqual(await session.feedRun('[Point.DIMS, Point.describe()]', { inputs: { Point: wrapper } }), [2, 'a point'])
  t.is(await session.feedRun('Point', { inputs: { Point: wrapper } }), Point)
  const constructed = await session.feedRun('Point(3, 4)', { inputs: { Point: wrapper } })
  t.true(constructed instanceof Point)
  t.is((constructed as Point).sum(), 7)
  t.true(await session.feedRun('type(Point(3, 4)) is Point', { inputs: { Point: wrapper } }))
})

test('a lazy attribute host error is raised in the sandbox over the wasm transport', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create()
  await using session = await pool.checkout({})

  const inputs = { f: new ClassInstance(new Flaky(), { lazyAttrs: 'all' }) }
  const code = "try:\n    f.boom\n    r = 'unexpected'\nexcept KeyError as e:\n    r = str(e)\nr"
  t.is(await session.feedRun(code, { inputs }), "'boom'")
  // only AttributeError is swallowed by hasattr, and the session stays usable
  const error = await t.throwsAsync(() => session.feedRun("hasattr(f, 'boom')", { inputs }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.message, 'KeyError: boom')
  t.is(await session.feedRun('f.value + 1', { inputs }), 2)
  // a value the arena cannot encode raises TypeError in the sandbox
  const unencodable = "try:\n    f.sym\n    r = 'unexpected'\nexcept TypeError as e:\n    r = str(e)\nr"
  t.is(await session.feedRun(unencodable, { inputs }), 'Cannot convert JS Symbol to Monty value')
})

test('a MontyClassProxy round-trips over the wasm transport', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create()
  await using session = await pool.checkout({})

  await session.feedRun('class Foo:\n    def __init__(self):\n        self.x = 1\nfoo = Foo()')
  const proxy = await session.feedRun('foo')
  t.true(proxy instanceof MontyClassProxy)
  t.is((proxy as MontyClassProxy).name, 'Foo')
  t.deepEqual({ ...(proxy as MontyClassProxy).attributes }, { x: 1 })
  t.true(await session.feedRun('back is foo and back.x == 1', { inputs: { back: proxy } }))
})
