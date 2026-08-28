// JavaScript value conversion for the semantic WASM component boundary.
//
// WIT cannot express recursive types, so values cross as a flat node arena.
// This file maps normal JavaScript values to and from that arena; all protobuf
// encoding, decoding, validation, and protocol dispatch now stays in Rust.

import { MontyFileHandle, canonicalFileMode, validateFilePosition } from '../types.js'
import type { NodePair, Value as ComponentValue, ValueNode } from './component/monty.component.js'

const I64_MIN = -(2n ** 63n)
const I64_MAX = 2n ** 63n - 1n
const SAFE = BigInt(Number.MAX_SAFE_INTEGER)
const TYPE_MARKER = '__monty_type__'

/** A non-enumerable marker stamped on arrays that came from Python tuples. */
export const TUPLE_MARKER = '__tuple__'

/** Converts a JavaScript value into the component's flat value arena. */
export function encodeValue(value: unknown): ComponentValue {
  const nodes: ValueNode[] = []
  const root = pushValue(value, nodes)
  return { root, nodes }
}

/** Converts a component value arena into its public JavaScript shape. */
export function decodeValue(value: ComponentValue): unknown {
  return readValue(value.root, value.nodes, new Set())
}

/** Appends one JavaScript value and its children, returning its node index. */
function pushValue(value: unknown, nodes: ValueNode[]): number {
  let node: ValueNode
  if (value === null || value === undefined) {
    node = { tag: 'none' }
  } else if (typeof value === 'boolean') {
    node = { tag: 'boolean', val: value }
  } else if (typeof value === 'number') {
    node =
      Number.isInteger(value) && (Number.isSafeInteger(value) || value === Number(I64_MIN))
        ? { tag: 'integer', val: BigInt(value) }
        : { tag: 'float', val: value }
  } else if (typeof value === 'bigint') {
    node =
      value >= I64_MIN && value <= I64_MAX ? { tag: 'integer', val: value } : { tag: 'bigint', val: value.toString() }
  } else if (typeof value === 'string') {
    node = { tag: 'text', val: value }
  } else if (value instanceof Uint8Array) {
    node = { tag: 'bytes', val: value }
  } else if (Array.isArray(value)) {
    const items = Uint32Array.from(value.map((item) => pushValue(item, nodes)))
    node = { tag: isTuple(value) ? 'tuple-value' : 'list-value', val: items }
  } else if (value instanceof Map) {
    node = { tag: 'dict', val: pushPairs([...value.entries()], nodes) }
  } else if (value instanceof Set) {
    node = { tag: 'set', val: Uint32Array.from([...value].map((item) => pushValue(item, nodes))) }
  } else if (typeof value === 'function') {
    node = { tag: 'function', val: { name: value.name ?? '' } }
  } else if (typeof value === 'object') {
    const object = value as Record<string, unknown>
    node =
      TYPE_MARKER in object ? pushMarked(object, nodes) : { tag: 'dict', val: pushPairs(Object.entries(object), nodes) }
  } else if (typeof value === 'symbol') {
    throw new TypeError('Cannot convert JS Symbol to Monty value')
  } else {
    throw unsupported(`value of type ${typeof value}`)
  }
  const index = nodes.length
  nodes.push(node)
  return index
}

/** Converts a `__monty_type__` marker into one semantic value node. */
function pushMarked(object: Record<string, unknown>, nodes: ValueNode[]): ValueNode {
  switch (object[TYPE_MARKER]) {
    case 'Ellipsis':
      return { tag: 'ellipsis' }
    case 'NotImplemented':
      return { tag: 'not-implemented' }
    case 'Date':
      return {
        tag: 'date',
        val: { year: Number(object.year), month: Number(object.month), day: Number(object.day) },
      }
    case 'DateTime':
      return {
        tag: 'datetime',
        val: {
          year: Number(object.year),
          month: Number(object.month),
          day: Number(object.day),
          hour: Number(object.hour),
          minute: Number(object.minute),
          second: Number(object.second),
          microsecond: Number(object.microsecond),
          ...timeZoneFields(object, 'DateTime'),
        },
      }
    case 'Time':
      return {
        tag: 'time',
        val: {
          hour: Number(object.hour),
          minute: Number(object.minute),
          second: Number(object.second),
          microsecond: Number(object.microsecond),
          ...timeZoneFields(object, 'Time'),
          fold: Number(object.fold ?? 0),
        },
      }
    case 'TimeDelta':
      return {
        tag: 'timedelta',
        val: {
          days: Number(object.days),
          seconds: Number(object.seconds),
          microseconds: Number(object.microseconds),
        },
      }
    case 'TimeZone':
      return {
        tag: 'timezone',
        val: {
          offsetSeconds: Number(object.offsetSeconds),
          ...(object.name === undefined ? {} : { name: String(object.name) }),
        },
      }
    case 'Exception':
      return {
        tag: 'exception',
        val: {
          excType: String(object.excType),
          ...(typeof object.message === 'string' ? { message: object.message } : {}),
        },
      }
    case 'Dataclass':
      return pushDataclass(object, nodes)
    case 'FileHandle':
      return pushFileHandle(object)
    case 'Type':
      return { tag: 'type-name', val: String(object.value) }
    case 'BuiltinFunction':
      return { tag: 'builtin-function', val: String(object.value) }
    default:
      throw new TypeError(`Unknown Monty marker type: ${String(object[TYPE_MARKER])}`)
  }
}

/** Preserves aware-time metadata while rejecting an orphaned timezone name. */
function timeZoneFields(
  object: Record<string, unknown>,
  typeName: 'DateTime' | 'Time',
): { offsetSeconds?: number; timezoneName?: string } {
  const aware = object.offsetSeconds !== undefined && object.offsetSeconds !== null
  if (!aware && object.timezoneName !== undefined && object.timezoneName !== null) {
    throw new TypeError(`Monty${typeName} timezoneName requires offsetSeconds`)
  }
  return aware
    ? {
        offsetSeconds: Number(object.offsetSeconds),
        ...(typeof object.timezoneName === 'string' ? { timezoneName: object.timezoneName } : {}),
      }
    : {}
}

/** Validates and converts a host dataclass marker. */
function pushDataclass(object: Record<string, unknown>, nodes: ValueNode[]): ValueNode {
  if (typeof object.typeId !== 'bigint') {
    throw new TypeError(
      `Object property 'typeId' type mismatch. Expect value to be BigInt, but received ${jsType(object.typeId)}`,
    )
  }
  if (!Array.isArray(object.fieldNames)) {
    throw new TypeError(
      `Object property 'fieldNames' type mismatch. Expect value to be Array, but received ${jsType(object.fieldNames)}`,
    )
  }
  const fields = (object.fields ?? {}) as Record<string, unknown>
  return {
    tag: 'dataclass',
    val: {
      name: String(object.name),
      typeId: object.typeId,
      fieldNames: object.fieldNames.map(String),
      attrs: pushPairs(Object.entries(fields), nodes),
      frozen: Boolean(object.frozen),
    },
  }
}

/** Validates and converts a sandbox file-handle marker. */
function pushFileHandle(object: Record<string, unknown>): ValueNode {
  if (typeof object.path !== 'string') throw new TypeError('MontyFileHandle path must be a string')
  if (typeof object.mode !== 'string') throw new TypeError('MontyFileHandle mode must be a string')
  const position = object.position === undefined ? 0 : object.position
  validateFilePosition(position)
  return {
    tag: 'file-handle',
    val: { path: object.path, mode: canonicalFileMode(object.mode), position: BigInt(position) },
  }
}

/** Appends key/value pairs while preserving their insertion order. */
function pushPairs(pairs: [unknown, unknown][], nodes: ValueNode[]): NodePair[] {
  return pairs.map(([key, value]) => ({ key: pushValue(key, nodes), value: pushValue(value, nodes) }))
}

/** Reads one arena node recursively, rejecting malformed indexes and cycles. */
function readValue(index: number, nodes: ValueNode[], visiting: Set<number>): unknown {
  const node = nodes[index]
  if (node === undefined) throw new Error(`component value node index ${index} is out of bounds`)
  if (visiting.has(index)) throw new Error('component value arena contains a cycle')
  visiting.add(index)
  let value: unknown
  switch (node.tag) {
    case 'ellipsis':
      value = { [TYPE_MARKER]: 'Ellipsis' }
      break
    case 'not-implemented':
      value = { [TYPE_MARKER]: 'NotImplemented' }
      break
    case 'none':
      value = null
      break
    case 'boolean':
    case 'float':
    case 'text':
      value = node.val
      break
    case 'integer':
      value = node.val >= -SAFE && node.val <= SAFE ? Number(node.val) : node.val
      break
    case 'bigint':
      value = BigInt(node.val)
      break
    case 'bytes':
      value = typeof Buffer === 'undefined' ? node.val : Buffer.from(node.val)
      break
    case 'list-value':
      value = readItems(node.val, nodes, visiting)
      break
    case 'tuple-value':
      value = asTuple(readItems(node.val, nodes, visiting))
      break
    case 'named-tuple':
      value = asTuple(readItems(node.val.items, nodes, visiting))
      break
    case 'dict':
      value = new Map(node.val.map((pair) => readPair(pair, nodes, visiting)))
      break
    case 'set':
    case 'frozen-set':
      value = new Set(readItems(node.val, nodes, visiting))
      break
    case 'date':
      value = { [TYPE_MARKER]: 'Date', ...node.val }
      break
    case 'datetime':
      value = { [TYPE_MARKER]: 'DateTime', ...node.val }
      break
    case 'time':
      value = { [TYPE_MARKER]: 'Time', ...node.val }
      break
    case 'timedelta':
      value = { [TYPE_MARKER]: 'TimeDelta', ...node.val }
      break
    case 'timezone':
      value = { [TYPE_MARKER]: 'TimeZone', ...node.val }
      break
    case 'exception':
      value = { [TYPE_MARKER]: 'Exception', excType: node.val.excType, message: node.val.message ?? '' }
      break
    case 'type-name':
    case 'instance-type':
      value = { [TYPE_MARKER]: 'Type', value: node.val }
      break
    case 'builtin-function':
      value = { [TYPE_MARKER]: 'BuiltinFunction', value: node.val }
      break
    case 'path':
    case 'repr':
      value = node.val
      break
    case 'file-handle':
      if (node.val.position > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new TypeError("MontyFileHandle position exceeds JavaScript's maximum safe integer")
      }
      value = new MontyFileHandle(node.val.path, node.val.mode, { position: Number(node.val.position) })
      break
    case 'dataclass':
      value = readDataclass(node.val, nodes, visiting)
      break
    case 'function':
      value = node.val.name
      break
    case 'cycle':
      value = node.val.placeholder
      break
  }
  visiting.delete(index)
  return value
}

/** Reads child indexes into a JavaScript array. */
function readItems(items: Uint32Array, nodes: ValueNode[], visiting: Set<number>): unknown[] {
  return [...items].map((index) => readValue(index, nodes, visiting))
}

/** Reads one indexed key/value pair. */
function readPair(pair: NodePair, nodes: ValueNode[], visiting: Set<number>): [unknown, unknown] {
  return [readValue(pair.key, nodes, visiting), readValue(pair.value, nodes, visiting)]
}

/** Rebuilds the public dataclass marker without prototype-setting assignment. */
function readDataclass(
  dataclass: Extract<ValueNode, { tag: 'dataclass' }>['val'],
  nodes: ValueNode[],
  visiting: Set<number>,
): Record<string, unknown> {
  const fields: Record<string, unknown> = {}
  for (const pair of dataclass.attrs) {
    const [key, value] = readPair(pair, nodes, visiting)
    if (typeof key === 'string') {
      Object.defineProperty(fields, key, { value, enumerable: true, writable: true, configurable: true })
    }
  }
  return {
    [TYPE_MARKER]: 'Dataclass',
    name: dataclass.name,
    typeId: dataclass.typeId,
    fieldNames: dataclass.fieldNames,
    fields,
    frozen: dataclass.frozen,
  }
}

/** Stamps the non-enumerable tuple marker used by both JS transports. */
function asTuple(items: unknown[]): unknown[] {
  Object.defineProperty(items, TUPLE_MARKER, { value: true, enumerable: false })
  return items
}

/** Whether an input array represents a Python tuple. */
function isTuple(array: unknown[]): boolean {
  return (array as { [TUPLE_MARKER]?: unknown })[TUPLE_MARKER] === true
}

/** Creates the established unsupported-value conversion error. */
function unsupported(what: string): Error {
  return new Error(`monty wasm transport does not support ${what}`)
}

/** Produces napi-compatible JavaScript type names for conversion errors. */
function jsType(value: unknown): string {
  if (value === undefined) {
    return 'Undefined'
  } else if (value === null) {
    return 'Null'
  } else if (Array.isArray(value)) {
    return 'Array'
  } else if (typeof value === 'bigint') {
    return 'BigInt'
  } else {
    return typeof value === 'object' ? 'Object' : typeof value
  }
}
