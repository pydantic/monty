// Object identity across the boundary. Values cross as one node arena per
// message, so a sub-object the sandbox references twice arrives as one host
// object, a host object passed twice is one sandbox object, and a cycle
// arrives as its placeholder string. Mirrors pydantic_monty's
// test_value_identity.py and runs on the native and wasm backends.

import { test } from 'vitest'
import { t } from './assertions.js'

import { ClassInstance, MontyClassProxy } from '@pydantic/monty'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

/** Counts the distinct arrays, Maps and Sets reachable from `value`. */
function distinctContainers(value: unknown): number {
  const seen = new Set<object>()
  const stack: unknown[] = [value]
  while (stack.length > 0) {
    const item = stack.pop()
    if (!(Array.isArray(item) || item instanceof Map || item instanceof Set) || seen.has(item)) {
      continue
    }
    seen.add(item)
    if (item instanceof Map) {
      stack.push(...item.keys(), ...item.values())
    } else {
      stack.push(...item)
    }
  }
  return seen.size
}

/** How many single-item arrays wrap the innermost value. */
function nesting(value: unknown): number {
  let depth = 0
  while (Array.isArray(value)) {
    value = value[0]
    depth += 1
  }
  return depth
}

// === sandbox → host ===

test('a shared child is one host object', async () => {
  const result = (await run('x = [1]\n[x, x]')) as unknown[]
  t.deepEqual(result, [[1], [1]])
  t.is(result[0], result[1])
})

test('a shared graph is linear in sandbox objects', async () => {
  // 36 lists that a tree export would expand into 753,663 nodes
  const code = `
x = [0]
for _ in range(20):
    x = [x]
for _ in range(15):
    x = [x, x]
x
`
  const result = (await run(code)) as unknown[]
  t.is(distinctContainers(result), 36)
  t.is(result[0], result[1])
})

test('a cycle arrives as its placeholder', async () => {
  t.deepEqual(await run('x = []\nx.append(x)\nx'), ['[...]'])
  t.deepEqual(await run("d = {}\nd['self'] = d\nd"), new Map([['self', '{...}']]))
})

test('a shared sandbox instance is one proxy', async () => {
  const result = (await run('class Foo:\n    pass\nfoo = Foo()\n[foo, foo]')) as unknown[]
  t.true(result[0] instanceof MontyClassProxy)
  t.is(result[0], result[1])
})

test('a deeply nested result crosses intact', async () => {
  t.is(nesting(await run('x = [1]\nfor _ in range(300):\n    x = [x]\nx')), 301)
})

// === host → sandbox ===

test('a shared child is one sandbox object', async () => {
  const y = [1]
  t.is(await run('xs[0] is xs[1]', { inputs: { xs: [y, y] } }), true)
})

test('an object shared across inputs is one sandbox object', async () => {
  const y = [1]
  t.is(await run('a is b', { inputs: { a: y, b: y } }), true)
})

test('a wrapper shared across inputs is one sandbox object', async () => {
  class Foo {}
  const foo = new Foo()
  const wrapper = new ClassInstance(foo)
  const result = (await run('[a is b, a]', { inputs: { a: wrapper, b: wrapper } })) as unknown[]
  t.is(result[0], true)
  t.is(result[1], foo)
})

test('a cyclic host return value raises TypeError in the sandbox', async () => {
  const f = () => {
    const x: unknown[] = []
    x.push(x)
    return x
  }
  const code = `
try:
    f()
    result = 'no error'
except TypeError as e:
    result = str(e)
result
`
  t.is(await run(code, { externalLookup: { f } }), 'Circular reference detected')
})

// === host functions ===

test('an argument passed twice is one host object', async () => {
  const seen: [unknown, unknown][] = []
  const f = (a: unknown, b: unknown) => {
    seen.push([a, b])
  }
  await using session = await pool().checkout()
  await session.feedRun('x = [1]\nf(x, x)', { externalLookup: { f } })
  t.deepEqual(seen, [[[1], [1]]])
  t.is(seen[0]![0], seen[0]![1])
})

test('separate calls get separate objects', async () => {
  const seen: unknown[] = []
  const f = (a: unknown) => {
    seen.push(a)
  }
  await using session = await pool().checkout()
  await session.feedRun('x = [1]\nf(x)\nf(x)', { externalLookup: { f } })
  t.deepEqual(seen, [[1], [1]])
  t.not(seen[0], seen[1])
})
