// Temporal values use optional WIT fields to distinguish naive values from
// aware values at UTC. These focused tests keep the semantic component codec
// aligned with the native conversion and Rust-side validation.

import { test } from 'vitest'

import { t } from './assertions.js'
import { encodeValue } from '../ts/worker/value.js'

function time(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { __monty_type__: 'Time', hour: 0, minute: 0, second: 0, microsecond: 0, ...extra }
}

test('the component codec preserves time timezone presence', () => {
  t.deepEqual(encodeValue(time()).nodes, [
    { tag: 'time', val: { hour: 0, minute: 0, second: 0, microsecond: 0, fold: 0 } },
  ])
  t.deepEqual(encodeValue(time({ fold: 1 })).nodes, [
    { tag: 'time', val: { hour: 0, minute: 0, second: 0, microsecond: 0, fold: 1 } },
  ])
  t.deepEqual(encodeValue(time({ offsetSeconds: 0 })).nodes, [
    { tag: 'time', val: { hour: 0, minute: 0, second: 0, microsecond: 0, offsetSeconds: 0, fold: 0 } },
  ])
  t.deepEqual(encodeValue(time({ offsetSeconds: 0, timezoneName: 'UTC' })).nodes, [
    {
      tag: 'time',
      val: { hour: 0, minute: 0, second: 0, microsecond: 0, offsetSeconds: 0, timezoneName: 'UTC', fold: 0 },
    },
  ])
})

test('the component codec rejects a timezone name with no offset', () => {
  t.throws(() => encodeValue(time({ timezoneName: 'UTC' })), {
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
  t.throws(() => encodeValue({ ...datetime, timezoneName: 'UTC' }), {
    instanceOf: TypeError,
    message: 'MontyDateTime timezoneName requires offsetSeconds',
  })
})
