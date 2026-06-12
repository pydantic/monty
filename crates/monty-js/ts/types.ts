// Marker-object shapes used by the native value conversion for Python types
// with no JS equivalent. The Rust side (src/convert.rs) produces and consumes
// objects of exactly these shapes; they are re-exported for users who want to
// construct (or type-check) such values in inputs, external function results,
// and `os` handlers.

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
