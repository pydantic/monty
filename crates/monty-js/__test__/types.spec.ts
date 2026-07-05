import test from 'ava'
import { Buffer } from 'node:buffer'

import { setupPool } from './helpers.js'

const { run } = setupPool(test)

// =============================================================================
// None tests
// =============================================================================

test('none input', async (t) => {
  t.is(await run('x is None', { inputs: { x: null } }), true)
})

test('none output', async (t) => {
  t.is(await run('None'), null)
})

// =============================================================================
// Bool tests
// =============================================================================

test('bool true', async (t) => {
  t.is(await run('x', { inputs: { x: true } }), true)
})

test('bool false', async (t) => {
  t.is(await run('x', { inputs: { x: false } }), false)
})

// =============================================================================
// Number tests
// =============================================================================

test('int', async (t) => {
  t.is(await run('x', { inputs: { x: 42 } }), 42)
  t.is(await run('x', { inputs: { x: -100 } }), -100)
  t.is(await run('x', { inputs: { x: 0 } }), 0)
})

test('float', async (t) => {
  t.is(await run('x', { inputs: { x: 3.14 } }), 3.14)
  t.is(await run('x', { inputs: { x: -2.5 } }), -2.5)
  t.is(await run('x', { inputs: { x: 0.0 } }), 0.0)
})

// =============================================================================
// String tests
// =============================================================================

test('string', async (t) => {
  t.is(await run('x', { inputs: { x: 'hello' } }), 'hello')
  t.is(await run('x', { inputs: { x: '' } }), '')
  t.is(await run('x', { inputs: { x: 'unicode: éè' } }), 'unicode: éè')
})

// =============================================================================
// Bytes tests
// =============================================================================

test('bytes', async (t) => {
  const result = await run('x', { inputs: { x: Buffer.from('hello') } })
  t.true(Buffer.isBuffer(result))
  t.deepEqual([...(result as Buffer)], [104, 101, 108, 108, 111])
})

test('bytes empty', async (t) => {
  const result = await run('x', { inputs: { x: Buffer.from([]) } })
  t.true(Buffer.isBuffer(result))
  t.deepEqual([...(result as Buffer)], [])
})

test('bytes result', async (t) => {
  const result = await run('b"hello"')
  t.true(Buffer.isBuffer(result))
  t.deepEqual([...(result as Buffer)], [104, 101, 108, 108, 111])
})

// =============================================================================
// str.encode('ascii') / bytes.decode('ascii') tests
// =============================================================================

test('str encode ascii ignore then bytes decode ascii round-trips', async (t) => {
  const result = await run('"café — 日本語 test".encode("ascii", "ignore").decode("ascii")')
  t.is(result, 'caf   test')
})

test('str encode ascii replace', async (t) => {
  const result = await run('"héllo".encode("ascii", "replace")')
  t.true(Buffer.isBuffer(result))
  t.deepEqual([...(result as Buffer)], [...Buffer.from('h?llo')])
})

test('bytes decode ascii backslashreplace', async (t) => {
  const result = await run('b"h\\xe9llo".decode("ascii", "backslashreplace")')
  t.is(result, 'h\\xe9llo')
})

// =============================================================================
// List tests
// =============================================================================

test('list', async (t) => {
  t.deepEqual(await run('x', { inputs: { x: [1, 2, 3] } }), [1, 2, 3])
  t.deepEqual(await run('x', { inputs: { x: [] } }), [])
  t.deepEqual(await run('x', { inputs: { x: ['a', 'b'] } }), ['a', 'b'])
})

test('list output', async (t) => {
  t.deepEqual(await run('[1, 2, 3]'), [1, 2, 3])
})

// =============================================================================
// Tuple tests
// =============================================================================

test('tuple', async (t) => {
  const result = await run('(1, 2, 3)')
  // Tuples are returned as arrays with a non-enumerable __tuple__ marker property
  t.true(Array.isArray(result))
  t.deepEqual(result, [1, 2, 3])
  t.is((result as any).__tuple__, true)
})

test('tuple empty', async (t) => {
  const result = await run('()')
  t.true(Array.isArray(result))
  t.deepEqual(result, [])
  t.is((result as any).__tuple__, true)
})

// =============================================================================
// Dict tests
// =============================================================================

test('dict', async (t) => {
  const result = await run('{"a": 1, "b": 2}')
  // Dicts are returned as native JS Map (preserves key types and insertion order)
  t.true(result instanceof Map)
  const map = result as Map<string, number>
  t.is(map.get('a'), 1)
  t.is(map.get('b'), 2)
  t.is(map.size, 2)
})

test('dict empty', async (t) => {
  const result = await run('{}')
  t.true(result instanceof Map)
  t.is((result as Map<unknown, unknown>).size, 0)
})

// =============================================================================
// Set tests
// =============================================================================

test('set', async (t) => {
  t.deepEqual(await run('{1, 2, 3}'), new Set([1, 2, 3]))
})

test('set empty', async (t) => {
  t.deepEqual(await run('set()'), new Set())
})

// =============================================================================
// Frozenset tests
// =============================================================================

test('frozenset', async (t) => {
  const result = await run('frozenset([1, 2, 3])')
  // FrozenSet is returned as a native JS Set (no frozen equivalent in JS)
  t.true(result instanceof Set)
  t.deepEqual(result, new Set([1, 2, 3]))
})

test('frozenset empty', async (t) => {
  t.deepEqual(await run('frozenset()'), new Set())
})

// =============================================================================
// Ellipsis tests
// =============================================================================

test('ellipsis input', async (t) => {
  // In JS we represent ellipsis as an object with __monty_type__: 'Ellipsis'
  t.is(await run('x is ...', { inputs: { x: { __monty_type__: 'Ellipsis' } } }), true)
})

test('ellipsis output', async (t) => {
  t.deepEqual(await run('...'), { __monty_type__: 'Ellipsis' })
})

// =============================================================================
// Nested collection tests
// =============================================================================

test('nested list', async (t) => {
  const nested = [
    [1, 2],
    [3, [4, 5]],
  ]
  t.deepEqual(await run('x', { inputs: { x: nested } }), [
    [1, 2],
    [3, [4, 5]],
  ])
})

test('nested dict', async (t) => {
  const result = await run('{"list": [1, 2], "nested": {"a": 1}}')
  // Dicts are returned as native JS Map
  t.true(result instanceof Map)
  const map = result as Map<string, unknown>
  t.deepEqual(map.get('list'), [1, 2])
  const nested = map.get('nested')
  t.true(nested instanceof Map)
  t.is((nested as Map<string, number>).get('a'), 1)
})

test('mixed nested', async (t) => {
  const result = await run('{"list": [1, 2], "tuple": (3, 4), "nested": {"set": {5, 6}}}')
  t.true(result instanceof Map)
  const map = result as Map<string, unknown>
  t.deepEqual(map.get('list'), [1, 2])
  const tuple = map.get('tuple')
  t.true(Array.isArray(tuple))
  t.is((tuple as any).__tuple__, true)
  t.deepEqual(tuple, [3, 4])
  const nested = map.get('nested')
  t.true(nested instanceof Map)
  t.true((nested as Map<string, unknown>).get('set') instanceof Set)
})

test('nested set in list', async (t) => {
  const result = await run('[{1, 2}, {3, 4}]')
  t.true(Array.isArray(result))
  const list = result as unknown[]
  t.is(list.length, 2)
  t.true(list[0] instanceof Set)
  t.true(list[1] instanceof Set)
  t.deepEqual(list[0], new Set([1, 2]))
  t.deepEqual(list[1], new Set([3, 4]))
})

test('nested bytes in dict', async (t) => {
  const result = await run('{"data": b"abc"}')
  t.true(result instanceof Map)
  const data = (result as Map<string, unknown>).get('data')
  t.true(Buffer.isBuffer(data))
  t.deepEqual([...(data as Buffer)], [97, 98, 99])
})

test('tuple containing set', async (t) => {
  const result = await run('({1, 2}, "hello")')
  t.true(Array.isArray(result))
  t.is((result as any).__tuple__, true)
  const tuple = result as unknown[]
  t.true(tuple[0] instanceof Set)
  t.deepEqual(tuple[0], new Set([1, 2]))
  t.is(tuple[1], 'hello')
})

// =============================================================================
// BigInt tests
// =============================================================================

test('bigint input', async (t) => {
  const big = 2n ** 100n
  t.is(await run('x', { inputs: { x: big } }), big)
})

test('bigint output', async (t) => {
  t.is(await run('2**100'), 2n ** 100n)
})

test('bigint negative input', async (t) => {
  const bigNeg = -(2n ** 100n)
  t.is(await run('x', { inputs: { x: bigNeg } }), bigNeg)
})

test('int overflow to bigint', async (t) => {
  const maxI64 = 9223372036854775807n
  t.is(await run('x + 1', { inputs: { x: maxI64 } }), maxI64 + 1n)
})

test('bigint arithmetic', async (t) => {
  const big = 2n ** 100n
  t.is(await run('x * 2 + y', { inputs: { x: big, y: big } }), big * 2n + big)
})

test('bigint comparison', async (t) => {
  const big = 2n ** 100n
  t.is(await run('x > y', { inputs: { x: big, y: 42 } }), true)
  t.is(await run('x > y', { inputs: { x: 42, y: big } }), false)
})

test('bigint in collection', async (t) => {
  const big = 2n ** 100n
  t.deepEqual(await run('x', { inputs: { x: [big, 42, big * 2n] } }), [big, 42, big * 2n])
})

test('number at the i64 boundary', async (t) => {
  // 2^63 is f64-representable but overflows i64, so it crosses as a float;
  // -2^63 is a valid i64 and stays an int
  t.is(await run('type(x).__name__', { inputs: { x: 2 ** 63 } }), 'float')
  t.is(await run('type(x).__name__', { inputs: { x: -(2 ** 63) } }), 'int')
  // ints beyond ±2^53 come back as BigInt
  t.is(await run('x', { inputs: { x: -(2 ** 63) } }), -(2n ** 63n))
})
