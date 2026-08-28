// `ts/worker/value.ts` is a second, hand-written encoder for the same wire
// format as `monty-proto`, and nothing but these tests holds the two together.
// A round-trip through a real session cannot catch a divergence here: the Rust
// decoder accepts a redundant zero-valued field just as happily as its absence,
// so the two transports can send different bytes and both work — until anything
// compares them.

import { test } from 'vitest'

import { t } from './assertions.js'
import { Reader } from '../ts/worker/proto.js'
import { encodeMontyObject } from '../ts/worker/value.js'

/** The proto field numbers written into an encoded value's submessage body. */
function fieldsOf(value: unknown): number[] {
  const body = new Reader(encodeMontyObject(value)).next().bytes
  const fields = new Reader(body)
  const present: number[] = []
  while (!fields.done) present.push(fields.next().field)
  return present
}

function time(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { __monty_type__: 'Time', hour: 0, minute: 0, second: 0, microsecond: 0, ...extra }
}

test('the TS codec follows the same proto3 presence rules as monty-proto', () => {
  // implicit presence: midnight is every field at its default, so the body is
  // empty, and `fold` is written only when set
  t.deepEqual(fieldsOf(time()), [])
  t.deepEqual(fieldsOf(time({ fold: 0 })), [])
  t.deepEqual(fieldsOf(time({ fold: 1 })), [7])
  t.deepEqual(fieldsOf(time({ hour: 1, microsecond: 5 })), [1, 4])

  // explicit presence: a UTC-aware time writes its zero offset, which is the
  // only thing on the wire distinguishing it from a naive one
  t.deepEqual(fieldsOf(time({ offsetSeconds: 0 })), [5])
  t.deepEqual(fieldsOf(time({ offsetSeconds: 0, timezoneName: 'UTC' })), [5, 6])

  // `TimeZone.offset_seconds` is a plain int32 rather than an `optional` one, so
  // here the same zero offset is carried by the field's absence
  t.deepEqual(fieldsOf({ __monty_type__: 'TimeZone', offsetSeconds: 0 }), [])
  t.deepEqual(fieldsOf({ __monty_type__: 'TimeZone', offsetSeconds: 0, name: 'UTC' }), [2])
  t.deepEqual(fieldsOf({ __monty_type__: 'TimeZone', offsetSeconds: 3600 }), [1])

  t.deepEqual(fieldsOf({ __monty_type__: 'TimeDelta', days: 0, seconds: 0, microseconds: 0 }), [])
  t.deepEqual(fieldsOf({ __monty_type__: 'TimeDelta', days: 0, seconds: 1, microseconds: 0 }), [2])
})

test('the TS codec rejects a timezone name with no offset', () => {
  // dropping the name would encode a different value; `monty-proto` rejects the
  // combination on decode rather than accepting it
  t.throws(() => encodeMontyObject(time({ timezoneName: 'UTC' })), {
    instanceOf: TypeError,
    message: 'MontyTime timezoneName requires offsetSeconds',
  })
  const datetime = {
    __monty_type__: 'DateTime',
    year: 2026,
    month: 1,
    day: 1,
    hour: 0,
    minute: 0,
    second: 0,
    microsecond: 0,
  }
  t.throws(() => encodeMontyObject({ ...datetime, timezoneName: 'UTC' }), {
    instanceOf: TypeError,
    message: 'MontyDateTime timezoneName requires offsetSeconds',
  })
})
