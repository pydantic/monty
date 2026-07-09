import { test } from 'vitest'
import { t } from './assertions.js'

import { Monty } from '../ts/wasm.js'
import { Buffer } from 'node:buffer'

// =============================================================================
// None tests
// =============================================================================

test('none input', () => {
  const m = new Monty('x is None', { inputs: ['x'] })
  t.is(m.run({ inputs: { x: null } }), true)
})

test('none output', () => {
  const m = new Monty('None')
  t.is(m.run(), null)
})

// =============================================================================
// Bool tests
// =============================================================================

test('bool true', () => {
  const m = new Monty('x', { inputs: ['x'] })
  const result = m.run({ inputs: { x: true } })
  t.is(result, true)
})

test('bool false', () => {
  const m = new Monty('x', { inputs: ['x'] })
  const result = m.run({ inputs: { x: false } })
  t.is(result, false)
})

// =============================================================================
// Number tests
// =============================================================================

test('int', () => {
  const m = new Monty('x', { inputs: ['x'] })
  t.is(m.run({ inputs: { x: 42 } }), 42)
  t.is(m.run({ inputs: { x: -100 } }), -100)
  t.is(m.run({ inputs: { x: 0 } }), 0)
})

test('float', () => {
  const m = new Monty('x', { inputs: ['x'] })
  t.is(m.run({ inputs: { x: 3.14 } }), 3.14)
  t.is(m.run({ inputs: { x: -2.5 } }), -2.5)
  t.is(m.run({ inputs: { x: 0.0 } }), 0.0)
})

// =============================================================================
// String tests
// =============================================================================

test('string', () => {
  const m = new Monty('x', { inputs: ['x'] })
  t.is(m.run({ inputs: { x: 'hello' } }), 'hello')
  t.is(m.run({ inputs: { x: '' } }), '')
  t.is(m.run({ inputs: { x: 'unicode: éè' } }), 'unicode: éè')
})

// =============================================================================
// Bytes tests
// =============================================================================

test('bytes', () => {
  const m = new Monty('x', { inputs: ['x'] })
  const result = m.run({ inputs: { x: Buffer.from('hello') } })
  t.true(Buffer.isBuffer(result))
  t.deepEqual([...result], [104, 101, 108, 108, 111])
})

test('bytes empty', () => {
  const m = new Monty('x', { inputs: ['x'] })
  const result = m.run({ inputs: { x: Buffer.from([]) } })
  t.true(Buffer.isBuffer(result))
  t.deepEqual([...result], [])
})

test('bytes result', () => {
  const m = new Monty('b"hello"')
  const result = m.run()
  t.true(Buffer.isBuffer(result))
  t.deepEqual([...result], [104, 101, 108, 108, 111])
})

// =============================================================================
// List tests
// =============================================================================

test('list', () => {
  const m = new Monty('x', { inputs: ['x'] })
  t.deepEqual(m.run({ inputs: { x: [1, 2, 3] } }), [1, 2, 3])
  t.deepEqual(m.run({ inputs: { x: [] } }), [])
  t.deepEqual(m.run({ inputs: { x: ['a', 'b'] } }), ['a', 'b'])
})

test('list output', () => {
  const m = new Monty('[1, 2, 3]')
  t.deepEqual(m.run(), [1, 2, 3])
})

// =============================================================================
// Tuple tests
// =============================================================================

test('tuple', () => {
  const m = new Monty('(1, 2, 3)')
  const result = m.run()
  // Tuples are returned as arrays with a __tuple__ marker property
  t.true(Array.isArray(result))
  t.deepEqual([...result], [1, 2, 3])
  t.is(result.__tuple__, true)
})

test('tuple empty', () => {
  const m = new Monty('()')
  const result = m.run()
  t.true(Array.isArray(result))
  t.deepEqual([...result], [])
  t.is(result.__tuple__, true)
})

// =============================================================================
// Dict tests
// =============================================================================

test('dict', () => {
  const m = new Monty('{"a": 1, "b": 2}')
  const result = m.run()
  // Dicts are returned as native JS Map (preserves key types and insertion order)
  t.true(result instanceof Map)
  t.is(result.get('a'), 1)
  t.is(result.get('b'), 2)
  t.is(result.size, 2)
})

test('dict empty', () => {
  const m = new Monty('{}')
  const result = m.run()
  t.true(result instanceof Map)
  t.is(result.size, 0)
})

// =============================================================================
// Set tests
// =============================================================================

test('set', () => {
  const m = new Monty('{1, 2, 3}')
  const result = m.run()
  t.deepEqual(result, new Set([1, 2, 3]))
})

test('set empty', () => {
  const m = new Monty('set()')
  const result = m.run()
  t.deepEqual(result, new Set())
})

// =============================================================================
// Frozenset tests
// =============================================================================

test('frozenset', () => {
  const m = new Monty('frozenset([1, 2, 3])')
  const result = m.run()
  // FrozenSet is returned as a native JS Set (no frozen equivalent in JS)
  t.true(result instanceof Set)
  t.deepEqual(result, new Set([1, 2, 3]))
})

test('frozenset empty', () => {
  const m = new Monty('frozenset()')
  const result = m.run()
  t.deepEqual(result, new Set())
})

// =============================================================================
// Ellipsis tests
// =============================================================================

test('ellipsis input', () => {
  // In JS we represent ellipsis as an object with __monty_type__: 'Ellipsis'
  const m = new Monty('x is ...', { inputs: ['x'] })
  t.is(m.run({ inputs: { x: { __monty_type__: 'Ellipsis' } } }), true)
})

test('ellipsis output', () => {
  const m = new Monty('...')
  const result = m.run()
  t.deepEqual(result, { __monty_type__: 'Ellipsis' })
})

// =============================================================================
// Nested collection tests
// =============================================================================

test('nested list', () => {
  const m = new Monty('x', { inputs: ['x'] })
  const nested = [
    [1, 2],
    [3, [4, 5]],
  ]
  t.deepEqual(m.run({ inputs: { x: nested } }), [
    [1, 2],
    [3, [4, 5]],
  ])
})

test('nested dict', () => {
  const m = new Monty('{"list": [1, 2], "nested": {"a": 1}}')
  const result = m.run()
  // Dicts are returned as native JS Map
  t.true(result instanceof Map)
  t.deepEqual(result.get('list'), [1, 2])
  const nested = result.get('nested')
  t.true(nested instanceof Map)
  t.is(nested.get('a'), 1)
})

test('mixed nested', () => {
  const m = new Monty('{"list": [1, 2], "tuple": (3, 4), "nested": {"set": {5, 6}}}')
  const result = m.run()
  t.true(result instanceof Map)
  t.deepEqual(result.get('list'), [1, 2])
  const tuple = result.get('tuple')
  t.true(Array.isArray(tuple))
  t.is(tuple.__tuple__, true)
  t.deepEqual([...tuple], [3, 4])
  const nested = result.get('nested')
  t.true(nested instanceof Map)
  t.true(nested.get('set') instanceof Set)
})

test('nested set in list', () => {
  const m = new Monty('[{1, 2}, {3, 4}]')
  const result = m.run()
  t.true(Array.isArray(result))
  t.is(result.length, 2)
  t.true(result[0] instanceof Set)
  t.true(result[1] instanceof Set)
  t.deepEqual(result[0], new Set([1, 2]))
  t.deepEqual(result[1], new Set([3, 4]))
})

test('nested bytes in dict', () => {
  const m = new Monty('{"data": b"abc"}')
  const result = m.run()
  t.true(result instanceof Map)
  const data = result.get('data')
  t.true(Buffer.isBuffer(data))
  t.deepEqual([...data], [97, 98, 99])
})

test('tuple containing set', () => {
  const m = new Monty('({1, 2}, "hello")')
  const result = m.run()
  t.true(Array.isArray(result))
  t.is(result.__tuple__, true)
  t.true(result[0] instanceof Set)
  t.deepEqual(result[0], new Set([1, 2]))
  t.is(result[1], 'hello')
})

// =============================================================================
// BigInt tests
// =============================================================================

test('bigint input', () => {
  const big = 2n ** 100n
  const m = new Monty('x', { inputs: ['x'] })
  const result = m.run({ inputs: { x: big } })
  t.is(result, big)
})

test('bigint output', () => {
  const m = new Monty('2**100')
  const result = m.run()
  t.is(result, 2n ** 100n)
})

test('bigint negative input', () => {
  const bigNeg = -(2n ** 100n)
  const m = new Monty('x', { inputs: ['x'] })
  const result = m.run({ inputs: { x: bigNeg } })
  t.is(result, bigNeg)
})

test('int overflow to bigint', () => {
  const maxI64 = 9223372036854775807n
  const m = new Monty('x + 1', { inputs: ['x'] })
  const result = m.run({ inputs: { x: maxI64 } })
  t.is(result, maxI64 + 1n)
})

test('bigint arithmetic', () => {
  const big = 2n ** 100n
  const m = new Monty('x * 2 + y', { inputs: ['x', 'y'] })
  const result = m.run({ inputs: { x: big, y: big } })
  t.is(result, big * 2n + big)
})

test('bigint comparison', () => {
  const big = 2n ** 100n
  const m = new Monty('x > y', { inputs: ['x', 'y'] })
  t.is(m.run({ inputs: { x: big, y: 42 } }), true)
  t.is(m.run({ inputs: { x: 42, y: big } }), false)
})

test('bigint in collection', () => {
  const big = 2n ** 100n
  const m = new Monty('x', { inputs: ['x'] })
  const result = m.run({ inputs: { x: [big, 42, big * 2n] } })
  t.deepEqual(result, [big, 42, big * 2n])
})
