// `MontyObject` <-> JS value conversion for the wasm worker transport.
//
// This mirrors the napi conversion in `crates/monty-js/src/convert.rs`: the
// browser path must hand back exactly the same JS shapes the native path does,
// so `MontySession`'s drive loop and user code see no difference between
// transports. Native JS types are used where they exist (number/BigInt/string/
// Buffer/Array/Map/Set); the `__tuple__` marker distinguishes tuples from
// lists and `__monty_type__` marks types with no JS equivalent.
//
// Scope: scalars, containers, the datetime family, named tuples (→ plain
// tuple), dataclasses, function values (→ name), and the marker types. File
// handles are NOT ported — convert.rs renders them as a Rust-side repr string
// this layer cannot reproduce — and throw a clear error rather than producing a
// wrong value. See `convert.rs` for the full mapping (plan item: TS decoder).

import { Reader, Writer, bitsToDouble, readInt32, unzigzag } from './proto.js'

// MontyObject oneof field numbers (monty.v1.MontyObject).
const Tag = {
  Ellipsis: 1,
  None: 2,
  Bool: 3,
  Int: 4,
  BigInt: 5,
  Float: 6,
  Str: 7,
  Bytes: 8,
  List: 9,
  Tuple: 10,
  NamedTuple: 11,
  Dict: 12,
  Set: 13,
  FrozenSet: 14,
  Date: 15,
  DateTime: 16,
  TimeDelta: 17,
  TimeZone: 18,
  Exception: 19,
  Type: 20,
  BuiltinFunction: 21,
  Path: 22,
  Dataclass: 24,
  Function: 25,
  Repr: 26,
  Cycle: 27,
} as const

const I64_MIN = -(2n ** 63n)
const I64_MAX = 2n ** 63n - 1n
const SAFE = BigInt(Number.MAX_SAFE_INTEGER)

/** A non-enumerable marker stamped on arrays that came from Python tuples. */
export const TUPLE_MARKER = '__tuple__'

// === encode: JS -> MontyObject message bytes ===

/** Encodes a JS value to the bytes of one `MontyObject` message. */
export function encodeMontyObject(value: unknown): Uint8Array {
  const w = new Writer()
  writeKind(w, value)
  return w.finish()
}

function writeKind(w: Writer, value: unknown): void {
  if (value === null || value === undefined) {
    w.lengthDelimited(Tag.None, EMPTY)
  } else if (typeof value === 'boolean') {
    w.bool(Tag.Bool, value)
  } else if (typeof value === 'number') {
    if (Number.isInteger(value) && (Number.isSafeInteger(value) || value === Number(I64_MIN))) {
      w.sint64(Tag.Int, BigInt(value))
    } else {
      w.double(Tag.Float, value)
    }
  } else if (typeof value === 'bigint') {
    if (value >= I64_MIN && value <= I64_MAX) {
      w.sint64(Tag.Int, value)
    } else {
      w.lengthDelimited(Tag.BigInt, encodeBigInt(value))
    }
  } else if (typeof value === 'string') {
    w.string(Tag.Str, value)
  } else if (value instanceof Uint8Array) {
    w.bytes(Tag.Bytes, value)
  } else if (Array.isArray(value)) {
    w.lengthDelimited(isTuple(value) ? Tag.Tuple : Tag.List, encodeList(value))
  } else if (value instanceof Map) {
    w.lengthDelimited(Tag.Dict, encodeDict([...value.entries()]))
  } else if (value instanceof Set) {
    w.lengthDelimited(Tag.Set, encodeList([...value.values()]))
  } else if (typeof value === 'function') {
    w.lengthDelimited(Tag.Function, encodeFunction(value as { name?: string }))
  } else if (typeof value === 'object') {
    const obj = value as Record<string, unknown>
    if (TYPE_MARKER in obj) {
      writeMarked(w, obj)
    } else {
      // a plain object becomes a string-keyed dict, matching convert.rs
      w.lengthDelimited(Tag.Dict, encodeDict(Object.entries(obj)))
    }
  } else if (typeof value === 'symbol') {
    throw new TypeError('Cannot convert JS Symbol to Monty value')
  } else {
    throw unsupported(`value of type ${typeof value}`)
  }
}

/** Encodes a `__monty_type__`-marked value back to its `MontyObject` kind. */
function writeMarked(w: Writer, obj: Record<string, unknown>): void {
  const type = obj[TYPE_MARKER]
  switch (type) {
    case 'Ellipsis':
      w.lengthDelimited(Tag.Ellipsis, EMPTY)
      break
    case 'Date':
      w.lengthDelimited(Tag.Date, encodeDate(obj))
      break
    case 'DateTime':
      w.lengthDelimited(Tag.DateTime, encodeDateTime(obj))
      break
    case 'TimeDelta':
      w.lengthDelimited(Tag.TimeDelta, encodeTimeDelta(obj))
      break
    case 'TimeZone':
      w.lengthDelimited(Tag.TimeZone, encodeTimeZone(obj))
      break
    case 'Exception':
      w.lengthDelimited(Tag.Exception, encodeExceptionValue(obj))
      break
    case 'Dataclass':
      w.lengthDelimited(Tag.Dataclass, encodeDataclass(obj))
      break
    case 'Type':
      w.string(Tag.Type, String(obj.value))
      break
    case 'BuiltinFunction':
      w.string(Tag.BuiltinFunction, String(obj.value))
      break
    default:
      throw unsupported(`marked value ${String(type)}`)
  }
}

function encodeFunction(value: { name?: string }): Uint8Array {
  const w = new Writer()
  w.string(1, value.name ?? '') // Function.name
  return w.finish()
}

function encodeDataclass(obj: Record<string, unknown>): Uint8Array {
  if (typeof obj.typeId !== 'bigint') {
    throw new TypeError(
      `Object property 'typeId' type mismatch. Expect value to be BigInt, but received ${jsType(obj.typeId)}`,
    )
  }
  if (!Array.isArray(obj.fieldNames)) {
    throw new TypeError(
      `Object property 'fieldNames' type mismatch. Expect value to be Array, but received ${jsType(obj.fieldNames)}`,
    )
  }
  const w = new Writer()
  w.string(1, String(obj.name)) // Dataclass.name
  w.uint(2, obj.typeId) // Dataclass.type_id
  for (const fieldName of obj.fieldNames) w.string(3, String(fieldName)) // Dataclass.field_names
  const fields = (obj.fields ?? {}) as Record<string, unknown>
  w.lengthDelimited(4, encodeDict(Object.entries(fields))) // Dataclass.attrs
  if (obj.frozen) w.bool(5, true) // Dataclass.frozen
  return w.finish()
}

function encodeDate(obj: Record<string, unknown>): Uint8Array {
  const w = new Writer()
  w.int32(1, num(obj.year))
  w.uint(2, num(obj.month))
  w.uint(3, num(obj.day))
  return w.finish()
}

function encodeDateTime(obj: Record<string, unknown>): Uint8Array {
  const w = new Writer()
  w.int32(1, num(obj.year))
  w.uint(2, num(obj.month))
  w.uint(3, num(obj.day))
  w.uint(4, num(obj.hour))
  w.uint(5, num(obj.minute))
  w.uint(6, num(obj.second))
  w.uint(7, num(obj.microsecond))
  if (obj.offsetSeconds !== undefined && obj.offsetSeconds !== null) {
    w.int32(8, num(obj.offsetSeconds))
    if (typeof obj.timezoneName === 'string') w.string(9, obj.timezoneName)
  }
  return w.finish()
}

function encodeTimeDelta(obj: Record<string, unknown>): Uint8Array {
  const w = new Writer()
  w.int32(1, num(obj.days))
  w.int32(2, num(obj.seconds))
  w.int32(3, num(obj.microseconds))
  return w.finish()
}

function encodeTimeZone(obj: Record<string, unknown>): Uint8Array {
  const w = new Writer()
  w.int32(1, num(obj.offsetSeconds))
  if (typeof obj.name === 'string') w.string(2, obj.name)
  return w.finish()
}

function encodeExceptionValue(obj: Record<string, unknown>): Uint8Array {
  const w = new Writer()
  w.string(1, String(obj.excType))
  if (typeof obj.message === 'string') w.string(2, obj.message)
  return w.finish()
}

function encodeList(items: unknown[]): Uint8Array {
  const w = new Writer()
  for (const item of items) w.lengthDelimited(1, encodeMontyObject(item)) // ObjectList.items
  return w.finish()
}

function encodeDict(pairs: [unknown, unknown][]): Uint8Array {
  const w = new Writer()
  for (const [key, value] of pairs) {
    const pair = new Writer()
    pair.lengthDelimited(1, encodeMontyObject(key)) // Pair.key
    pair.lengthDelimited(2, encodeMontyObject(value)) // Pair.value
    w.lengthDelimited(1, pair.finish()) // Dict.pairs
  }
  return w.finish()
}

function encodeBigInt(value: bigint): Uint8Array {
  const w = new Writer()
  w.bool(1, value < 0n) // BigInt.negative
  let n = value < 0n ? -value : value
  const magnitude: number[] = []
  while (n > 0n) {
    magnitude.push(Number(n & 0xffn))
    n >>= 8n
  }
  magnitude.reverse() // big-endian
  w.bytes(2, Uint8Array.from(magnitude)) // BigInt.magnitude
  return w.finish()
}

// === decode: MontyObject message bytes -> JS ===

/** Decodes the bytes of one `MontyObject` message to a JS value. */
export function decodeMontyObject(bytes: Uint8Array): unknown {
  const reader = new Reader(bytes)
  if (reader.done) throw new Error('empty MontyObject')
  const f = reader.next()
  switch (f.field) {
    case Tag.Ellipsis:
      return { [TYPE_MARKER]: 'Ellipsis' }
    case Tag.None:
      return null
    case Tag.Bool:
      return f.value !== 0n
    case Tag.Int: {
      const n = unzigzag(f.value)
      return n >= -SAFE && n <= SAFE ? Number(n) : n
    }
    case Tag.BigInt:
      return decodeBigInt(f.bytes)
    case Tag.Float:
      return bitsToDouble(f.value)
    case Tag.Str:
    case Tag.Path:
    case Tag.Repr:
      return decodeString(f.bytes)
    case Tag.Bytes:
      return typeof Buffer === 'undefined' ? f.bytes : Buffer.from(f.bytes)
    case Tag.List:
      return decodeList(f.bytes)
    case Tag.Tuple:
      return asTuple(decodeList(f.bytes))
    case Tag.NamedTuple:
      // the JS representation discards field names (a plain tuple), matching convert.rs
      return asTuple(decodeNamedTupleValues(f.bytes))
    case Tag.Dict:
      return decodeDict(f.bytes)
    case Tag.Set:
    case Tag.FrozenSet:
      return new Set(decodeList(f.bytes))
    case Tag.Date:
      return decodeDate(f.bytes)
    case Tag.DateTime:
      return decodeDateTime(f.bytes)
    case Tag.TimeDelta:
      return decodeTimeDelta(f.bytes)
    case Tag.TimeZone:
      return decodeTimeZone(f.bytes)
    case Tag.Exception:
      return decodeException(f.bytes)
    case Tag.Dataclass:
      return decodeDataclass(f.bytes)
    case Tag.Type:
      return { [TYPE_MARKER]: 'Type', value: decodeString(f.bytes) }
    case Tag.BuiltinFunction:
      return { [TYPE_MARKER]: 'BuiltinFunction', value: decodeString(f.bytes) }
    case Tag.Function:
      return decodeStringField(f.bytes, 1) // Function.name -> the name string
    case Tag.Cycle:
      return decodeStringField(f.bytes, 2) // Cycle.placeholder
    default:
      throw unsupported(`MontyObject kind field ${f.field}`)
  }
}

function decodeNamedTupleValues(bytes: Uint8Array): unknown[] {
  const reader = new Reader(bytes)
  const values: unknown[] = []
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 3) values.push(decodeMontyObject(f.bytes)) // NamedTuple.values
  }
  return values
}

function decodeDataclass(bytes: Uint8Array): MarkedValue {
  const dataclass: MarkedValue = {
    [TYPE_MARKER]: 'Dataclass',
    name: '',
    typeId: 0n,
    fieldNames: [] as string[],
    fields: {} as Record<string, unknown>,
    frozen: false,
  }
  const fieldNames = dataclass.fieldNames as string[]
  const fields = dataclass.fields as Record<string, unknown>
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    switch (f.field) {
      case 1:
        dataclass.name = decodeString(f.bytes)
        break
      case 2:
        dataclass.typeId = f.value // uint64 -> BigInt
        break
      case 3:
        fieldNames.push(decodeString(f.bytes))
        break
      case 4:
        for (const [key, value] of decodeDict(f.bytes)) {
          // defineProperty (not assignment) so a "__proto__" field can't pollute
          if (typeof key === 'string') {
            Object.defineProperty(fields, key, { value, enumerable: true, writable: true, configurable: true })
          }
        }
        break
      case 5:
        dataclass.frozen = f.value !== 0n
        break
    }
  }
  return dataclass
}

function decodeStringField(bytes: Uint8Array, field: number): string {
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    if (f.field === field) return decodeString(f.bytes)
  }
  return ''
}

function decodeList(bytes: Uint8Array): unknown[] {
  const reader = new Reader(bytes)
  const items: unknown[] = []
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) items.push(decodeMontyObject(f.bytes)) // ObjectList.items
  }
  return items
}

function decodeDict(bytes: Uint8Array): Map<unknown, unknown> {
  const reader = new Reader(bytes)
  const map = new Map<unknown, unknown>()
  while (!reader.done) {
    const f = reader.next()
    if (f.field !== 1) continue // Dict.pairs
    let key: unknown = null
    let value: unknown = null
    const pair = new Reader(f.bytes)
    while (!pair.done) {
      const pf = pair.next()
      if (pf.field === 1) key = decodeMontyObject(pf.bytes)
      else if (pf.field === 2) value = decodeMontyObject(pf.bytes)
    }
    map.set(key, value)
  }
  return map
}

function decodeDate(bytes: Uint8Array): MarkedValue {
  const date: MarkedValue = { [TYPE_MARKER]: 'Date', year: 0, month: 0, day: 0 }
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) date.year = readInt32(f.value)
    else if (f.field === 2) date.month = Number(f.value)
    else if (f.field === 3) date.day = Number(f.value)
  }
  return date
}

function decodeDateTime(bytes: Uint8Array): MarkedValue {
  const dt: MarkedValue = {
    [TYPE_MARKER]: 'DateTime',
    year: 0,
    month: 0,
    day: 0,
    hour: 0,
    minute: 0,
    second: 0,
    microsecond: 0,
  }
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    switch (f.field) {
      case 1:
        dt.year = readInt32(f.value)
        break
      case 2:
        dt.month = Number(f.value)
        break
      case 3:
        dt.day = Number(f.value)
        break
      case 4:
        dt.hour = Number(f.value)
        break
      case 5:
        dt.minute = Number(f.value)
        break
      case 6:
        dt.second = Number(f.value)
        break
      case 7:
        dt.microsecond = Number(f.value)
        break
      case 8:
        dt.offsetSeconds = readInt32(f.value)
        break
      case 9:
        dt.timezoneName = decodeString(f.bytes)
        break
    }
  }
  return dt
}

function decodeTimeDelta(bytes: Uint8Array): MarkedValue {
  const td: MarkedValue = { [TYPE_MARKER]: 'TimeDelta', days: 0, seconds: 0, microseconds: 0 }
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) td.days = readInt32(f.value)
    else if (f.field === 2) td.seconds = readInt32(f.value)
    else if (f.field === 3) td.microseconds = readInt32(f.value)
  }
  return td
}

export function decodeTimeZone(bytes: Uint8Array): MarkedValue {
  const tz: MarkedValue = { [TYPE_MARKER]: 'TimeZone', offsetSeconds: 0 }
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) tz.offsetSeconds = readInt32(f.value)
    else if (f.field === 2) tz.name = decodeString(f.bytes)
  }
  return tz
}

function decodeException(bytes: Uint8Array): MarkedValue {
  const reader = new Reader(bytes)
  let excType = ''
  let message: string | undefined
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1)
      excType = decodeString(f.bytes) // Exception.exc_type
    else if (f.field === 2) message = decodeString(f.bytes) // Exception.arg
  }
  return { [TYPE_MARKER]: 'Exception', excType, message }
}

function decodeBigInt(bytes: Uint8Array): bigint {
  const reader = new Reader(bytes)
  let negative = false
  let magnitude: Uint8Array = EMPTY
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1)
      negative = f.value !== 0n // BigInt.negative
    else if (f.field === 2) magnitude = f.bytes // BigInt.magnitude
  }
  let n = 0n
  for (const b of magnitude) n = (n << 8n) | BigInt(b)
  return negative ? -n : n
}

// === helpers ===

const EMPTY = new Uint8Array(0)
const TYPE_MARKER = '__monty_type__'

interface MarkedValue {
  [TYPE_MARKER]: string
  [key: string]: unknown
}

function decodeString(bytes: Uint8Array): string {
  return new TextDecoder().decode(bytes)
}

/** Stamps the non-enumerable `__tuple__` marker, matching convert.rs. */
function asTuple(items: unknown[]): unknown[] {
  Object.defineProperty(items, TUPLE_MARKER, { value: true, enumerable: false })
  return items
}

function isTuple(array: unknown[]): boolean {
  return (array as { [TUPLE_MARKER]?: unknown })[TUPLE_MARKER] === true
}

/** Coerces a marked-value field (typed `unknown`) to a number for encoding. */
function num(value: unknown): number {
  return Number(value)
}

function unsupported(what: string): Error {
  return new Error(`monty wasm transport does not support ${what} (file handles are not yet ported)`)
}

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
