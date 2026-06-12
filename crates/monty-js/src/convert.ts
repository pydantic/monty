// Conversion between JavaScript values and wire `MontyValue`s, preserving
// the conventions of the original napi binding so the package's value model
// is unchanged:
//
// Native JS types (bidirectional):
// - none ↔ `null` (and `undefined` → none)
// - boolean ↔ `boolean`
// - int ↔ `number` within ±2^53, JS `BigInt` outside
// - bigint ↔ `BigInt`
// - float ↔ `number` (including NaN/±Infinity)
// - str ↔ `string`
// - bytes ↔ `Buffer` (any `Uint8Array` accepted as input)
// - list ↔ `Array`
// - dict ↔ `Map` (preserves key types and insertion order; plain objects are
//   accepted as input with string keys)
// - set / frozen_set ↔ `Set`
//
// Marked JS types (objects carrying `__monty_type__`, plus `__tuple__` on
// arrays): Ellipsis, Exception, Date, DateTime, TimeDelta, TimeZone, Type,
// BuiltinFunction, Dataclass, FileHandle. `repr` and `cycle` arrive as plain
// strings (output-only).

import { create } from '@bufbuild/protobuf'
import {
  BigIntValueSchema,
  DataclassValueSchema,
  DateTimeValueSchema,
  DateValueSchema,
  DictValueSchema,
  ExceptionValueSchema,
  FileHandleValueSchema,
  FunctionValueSchema,
  MontyValueSchema,
  PairSchema,
  TimeDeltaValueSchema,
  TimeZoneValueSchema,
  UnitSchema,
  ValueListSchema,
  type DictValue,
  type MontyValue,
  type Pair,
} from './generated/monty/v1/monty_pb.js'

/** JS exactly-representable integer bound (±2^53), matching the old binding. */
const JS_SAFE_INT = 2 ** 53

/** f64-representable i64 bound used by the old binding's number→int check. */
const I64_BOUND = 2 ** 63

/**
 * Deepest value nesting the wire protocol accepts, re-exported from
 * monty-proto. Deeper inputs are rejected before anything is sent (the child
 * could not decode them).
 */
export const MAX_VALUE_DEPTH = 48

/** Marker object representing a Python `datetime.date`. */
export interface MontyDate {
  __monty_type__: 'Date'
  year: number
  month: number
  day: number
}

/** Marker object representing a Python `datetime.datetime`. */
export interface MontyDateTime {
  __monty_type__: 'DateTime'
  year: number
  month: number
  day: number
  hour: number
  minute: number
  second: number
  microsecond: number
  offsetSeconds?: number
  timezoneName?: string
}

/** Marker object representing a Python `datetime.timedelta`. */
export interface MontyTimeDelta {
  __monty_type__: 'TimeDelta'
  days: number
  seconds: number
  microseconds: number
}

/** Marker object representing a Python `datetime.timezone`. */
export interface MontyTimeZone {
  __monty_type__: 'TimeZone'
  offsetSeconds: number
  name?: string
}

/** Marker object representing a Python exception value. */
export interface MontyException {
  __monty_type__: 'Exception'
  excType: string
  message: string
}

/** Marker object representing a sandbox file handle (used by `os` handlers). */
export interface MontyFileHandle {
  __monty_type__: 'FileHandle'
  path: string
  mode: string
  position: number
}

/** Thrown when a JS value has no Monty representation. */
export class ConversionError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'ConversionError'
  }
}

/** Shorthand for building a `MontyValue` with the given oneof arm. */
function value(kind: MontyValue['kind']): MontyValue {
  return create(MontyValueSchema, { kind })
}

function pair(key: MontyValue, val: MontyValue): Pair {
  return create(PairSchema, { key, value: val })
}

const UNIT = () => create(UnitSchema)

/**
 * Converts a JavaScript value to a wire `MontyValue`.
 *
 * Throws [`ConversionError`] for values with no Monty representation
 * (symbols, unrecognised platform objects).
 */
export function jsToMonty(js: unknown): MontyValue {
  if (js === null || js === undefined) {
    return value({ case: 'none', value: UNIT() })
  }
  switch (typeof js) {
    case 'boolean':
      return value({ case: 'boolean', value: js })
    case 'number':
      // half-open: 2^63 itself is f64-representable but overflows sint64,
      // while -2^63 is a valid i64
      if (Number.isInteger(js) && js >= -I64_BOUND && js < I64_BOUND) {
        return value({ case: 'int', value: BigInt(js) })
      }
      return value({ case: 'float', value: js })
    case 'bigint':
      return bigintToMonty(js)
    case 'string':
      return value({ case: 'str', value: js })
    case 'function':
      return value({
        case: 'function',
        value: create(FunctionValueSchema, { name: js.name || '<anonymous>' }),
      })
    case 'object':
      return objectToMonty(js)
    default:
      throw new ConversionError(`cannot convert JS ${typeof js} to a Monty value`)
  }
}

/** Integers in i64 range use the compact `int` arm; wider go as sign+magnitude. */
function bigintToMonty(js: bigint): MontyValue {
  if (js >= -(2n ** 63n) && js < 2n ** 63n) {
    return value({ case: 'int', value: js })
  }
  const negative = js < 0n
  const magnitude = negative ? -js : js
  let hex = magnitude.toString(16)
  if (hex.length % 2 === 1) {
    hex = `0${hex}`
  }
  return value({
    case: 'bigint',
    value: create(BigIntValueSchema, { negative, magnitude: Buffer.from(hex, 'hex') }),
  })
}

function objectToMonty(js: object): MontyValue {
  if (js instanceof Uint8Array) {
    return value({ case: 'bytes', value: new Uint8Array(js) })
  }
  if (Array.isArray(js)) {
    const items = js.map(jsToMonty)
    const list = create(ValueListSchema, { items })
    const isTuple = (js as unknown as { __tuple__?: boolean }).__tuple__ === true
    return value(isTuple ? { case: 'tuple', value: list } : { case: 'list', value: list })
  }
  if (js instanceof Map) {
    const pairs = [...js.entries()].map(([k, v]) => pair(jsToMonty(k), jsToMonty(v)))
    return value({ case: 'dict', value: create(DictValueSchema, { pairs }) })
  }
  if (js instanceof Set) {
    const items = [...js.values()].map(jsToMonty)
    return value({ case: 'set', value: create(ValueListSchema, { items }) })
  }
  const marker = (js as { __monty_type__?: unknown }).__monty_type__
  if (typeof marker === 'string') {
    return markedObjectToMonty(js as Record<string, unknown>, marker)
  }
  return plainObjectToDict(js as Record<string, unknown>)
}

/** Converts an object carrying a `__monty_type__` marker. */
function markedObjectToMonty(js: Record<string, unknown>, marker: string): MontyValue {
  switch (marker) {
    case 'Ellipsis':
      return value({ case: 'ellipsis', value: UNIT() })
    case 'Exception': {
      const message = String(js.message ?? '')
      return value({
        case: 'exception',
        value: create(ExceptionValueSchema, {
          excType: String(js.excType),
          ...(message === '' ? {} : { arg: message }),
        }),
      })
    }
    case 'Date':
      return value({
        case: 'date',
        value: create(DateValueSchema, { year: Number(js.year), month: Number(js.month), day: Number(js.day) }),
      })
    case 'DateTime':
      return value({
        case: 'datetime',
        value: create(DateTimeValueSchema, {
          year: Number(js.year),
          month: Number(js.month),
          day: Number(js.day),
          hour: Number(js.hour),
          minute: Number(js.minute),
          second: Number(js.second),
          microsecond: Number(js.microsecond),
          ...(js.offsetSeconds !== undefined ? { offsetSeconds: Number(js.offsetSeconds) } : {}),
          ...(js.timezoneName !== undefined ? { timezoneName: String(js.timezoneName) } : {}),
        }),
      })
    case 'TimeDelta':
      return value({
        case: 'timedelta',
        value: create(TimeDeltaValueSchema, {
          days: Number(js.days),
          seconds: Number(js.seconds),
          microseconds: Number(js.microseconds),
        }),
      })
    case 'TimeZone':
      return value({
        case: 'timezone',
        value: create(TimeZoneValueSchema, {
          offsetSeconds: Number(js.offsetSeconds),
          ...(js.name !== undefined ? { name: String(js.name) } : {}),
        }),
      })
    // Type and builtin-function objects cannot round-trip into the sandbox;
    // the old binding sent their repr, which the child rejects as input with
    // a Python-level error if actually used.
    case 'Type':
      return value({ case: 'repr', value: `<class '${String(js.value)}'>` })
    case 'BuiltinFunction':
      return value({ case: 'repr', value: `<built-in function ${String(js.value)}>` })
    case 'FileHandle':
      return value({
        case: 'fileHandle',
        value: create(FileHandleValueSchema, {
          path: String(js.path),
          mode: String(js.mode),
          position: BigInt((js.position as number | bigint | undefined) ?? 0),
        }),
      })
    case 'Dataclass': {
      // marker objects are arbitrary user input — malformed shapes must
      // surface as ConversionError, not as a raw TypeError from .map()
      if (!Array.isArray(js.fieldNames)) {
        throw new ConversionError('Dataclass marker requires a fieldNames array')
      }
      if (js.fields !== undefined && (typeof js.fields !== 'object' || js.fields === null)) {
        throw new ConversionError('Dataclass marker fields must be an object')
      }
      const fieldNames = (js.fieldNames as unknown[]).map(String)
      const fields = (js.fields ?? {}) as Record<string, unknown>
      const pairs = fieldNames
        .filter((name) => name in fields)
        .map((name) => pair(value({ case: 'str', value: name }), jsToMonty(fields[name])))
      return value({
        case: 'dataclass',
        value: create(DataclassValueSchema, {
          name: String(js.name),
          typeId: BigInt((js.typeId as number | bigint | undefined) ?? 0),
          fieldNames,
          attrs: create(DictValueSchema, { pairs }),
          frozen: js.frozen === true,
        }),
      })
    }
    default:
      // Unknown marker: treat as a plain dict, like the old binding.
      return plainObjectToDict(js)
  }
}

/** Plain objects become dicts with string keys (own enumerable properties). */
function plainObjectToDict(js: Record<string, unknown>): MontyValue {
  const pairs = Object.entries(js).map(([k, v]) => pair(value({ case: 'str', value: k }), jsToMonty(v)))
  return value({ case: 'dict', value: create(DictValueSchema, { pairs }) })
}

/**
 * Converts a wire `MontyValue` to a JavaScript value. Total — every wire
 * value (including the output-only `repr`/`cycle` arms) has a JS form.
 */
export function montyToJs(monty: MontyValue): unknown {
  const kind = monty.kind
  switch (kind.case) {
    case 'none':
      return null
    case 'ellipsis':
      return { __monty_type__: 'Ellipsis' }
    case 'boolean':
      return kind.value
    case 'int':
      return kind.value >= -JS_SAFE_INT && kind.value <= JS_SAFE_INT ? Number(kind.value) : kind.value
    case 'bigint': {
      const hex = Buffer.from(kind.value.magnitude).toString('hex')
      const magnitude = hex === '' ? 0n : BigInt(`0x${hex}`)
      return kind.value.negative ? -magnitude : magnitude
    }
    case 'float':
      return kind.value
    case 'str':
      return kind.value
    case 'bytes':
      return Buffer.from(kind.value)
    case 'list':
      return kind.value.items.map(montyToJs)
    case 'tuple':
      return tupleToJs(kind.value.items)
    case 'namedTuple':
      // Named access is lost in JS; named tuples become marked tuples.
      return tupleToJs(kind.value.values)
    case 'dict':
      return dictToJs(kind.value)
    case 'set':
    case 'frozenSet':
      return new Set(kind.value.items.map(montyToJs))
    case 'date': {
      const { year, month, day } = kind.value
      return { __monty_type__: 'Date', year, month, day } satisfies MontyDate
    }
    case 'datetime': {
      const v = kind.value
      return {
        __monty_type__: 'DateTime',
        year: v.year,
        month: v.month,
        day: v.day,
        hour: v.hour,
        minute: v.minute,
        second: v.second,
        microsecond: v.microsecond,
        ...(v.offsetSeconds !== undefined ? { offsetSeconds: v.offsetSeconds } : {}),
        ...(v.timezoneName !== undefined ? { timezoneName: v.timezoneName } : {}),
      } satisfies MontyDateTime
    }
    case 'timedelta': {
      const { days, seconds, microseconds } = kind.value
      return { __monty_type__: 'TimeDelta', days, seconds, microseconds } satisfies MontyTimeDelta
    }
    case 'timezone':
      return {
        __monty_type__: 'TimeZone',
        offsetSeconds: kind.value.offsetSeconds,
        ...(kind.value.name !== undefined ? { name: kind.value.name } : {}),
      } satisfies MontyTimeZone
    case 'exception':
      return {
        __monty_type__: 'Exception',
        excType: kind.value.excType,
        message: kind.value.arg ?? '',
      } satisfies MontyException
    case 'type':
      return { __monty_type__: 'Type', value: kind.value }
    case 'builtinFunction':
      return { __monty_type__: 'BuiltinFunction', value: kind.value }
    case 'path':
      return kind.value
    case 'fileHandle':
      return {
        __monty_type__: 'FileHandle',
        path: kind.value.path,
        mode: kind.value.mode,
        position: Number(kind.value.position),
      } satisfies MontyFileHandle
    case 'dataclass': {
      const v = kind.value
      const fields: Record<string, unknown> = {}
      for (const p of v.attrs?.pairs ?? []) {
        if (p.key?.kind.case === 'str' && p.value && v.fieldNames.includes(p.key.kind.value)) {
          fields[p.key.kind.value] = montyToJs(p.value)
        }
      }
      return {
        __monty_type__: 'Dataclass',
        name: v.name,
        typeId: v.typeId,
        fieldNames: [...v.fieldNames],
        fields,
        frozen: v.frozen,
      }
    }
    case 'function':
      // Internal to the name-lookup protocol; surfaces as the function name.
      return kind.value.name
    case 'repr':
      return kind.value
    case 'cycle':
      return kind.value.placeholder
    case undefined:
      throw new ConversionError('empty MontyValue received from worker')
  }
}

function tupleToJs(items: MontyValue[]): unknown[] {
  const arr = items.map(montyToJs)
  Object.defineProperty(arr, '__tuple__', { value: true, enumerable: false })
  return arr
}

/** Dicts become `Map`s, preserving key types and insertion order. */
function dictToJs(dict: DictValue): Map<unknown, unknown> {
  const map = new Map<unknown, unknown>()
  for (const p of dict.pairs) {
    if (p.key && p.value) {
      map.set(montyToJs(p.key), montyToJs(p.value))
    }
  }
  return map
}

/**
 * Whether `value` nests deeper than [`MAX_VALUE_DEPTH`]. Mirrors
 * monty-proto's budget-bounded walk: one budget level per container, bailing
 * out (without descending) once the budget is exhausted so the check itself
 * cannot overflow on adversarially deep values.
 */
export function exceedsMaxValueDepth(value: MontyValue): boolean {
  return depthExceeds(value, MAX_VALUE_DEPTH)
}

function depthExceeds(value: MontyValue, budget: number): boolean {
  const kind = value.kind
  switch (kind.case) {
    case 'list':
    case 'tuple':
    case 'set':
    case 'frozenSet':
      return seqExceeds(kind.value.items, budget)
    case 'namedTuple':
      return seqExceeds(kind.value.values, budget)
    case 'dict':
      return pairsExceed(kind.value.pairs, budget)
    case 'dataclass':
      return pairsExceed(kind.value.attrs?.pairs ?? [], budget)
    default:
      return false
  }
}

function seqExceeds(items: MontyValue[], budget: number): boolean {
  if (budget === 0) {
    return items.length > 0
  }
  return items.some((child) => depthExceeds(child, budget - 1))
}

function pairsExceed(pairs: Pair[], budget: number): boolean {
  if (budget === 0) {
    return pairs.length > 0
  }
  return pairs.some(
    (p) =>
      (p.key !== undefined && depthExceeds(p.key, budget - 1)) ||
      (p.value !== undefined && depthExceeds(p.value, budget - 1)),
  )
}
