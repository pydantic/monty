// JavaScript representations of Python types with no native JS equivalent.
// Internal markers let both transports preserve the exact Monty value kind,
// while public constructors hide that protocol detail from callers.

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

/** Marker object representing a Python `datetime.time`. */
export interface MontyTime {
  __monty_type__: 'Time'
  hour: number
  minute: number
  second: number
  microsecond: number
  offsetSeconds?: number
  timezoneName?: string
  /** Disambiguates a repeated wall clock; Monty stores it but never reads it. */
  fold?: number
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

/** Optional keyword-style arguments for [`MontyFileHandle`]. */
export interface MontyFileHandleOptions {
  /** Initial character or byte position; defaults to zero. */
  position?: number
}

/**
 * Host-side handle to a file opened inside a Monty sandbox.
 *
 * This is a plain data holder, never a live host file descriptor. Return one
 * from an `open` OS callback so Monty can construct its sandboxed file object.
 */
export class MontyFileHandle {
  /** Virtual POSIX sandbox path, never a host path. */
  readonly path: string
  /** Canonical Python `open()` mode, such as `'r'`, `'rb'`, or `'w'`. */
  readonly mode: string
  /** Current character or byte position. */
  readonly position: number

  /** Constructs a file handle to return from an `open` OS callback. */
  constructor(path: string, mode: string, options: MontyFileHandleOptions = {}) {
    if (typeof path !== 'string') throw new TypeError('MontyFileHandle path must be a string')
    if (typeof mode !== 'string') throw new TypeError('MontyFileHandle mode must be a string')
    const position = options.position ?? 0
    validateFilePosition(position)

    this.path = path
    this.mode = canonicalFileMode(mode)
    this.position = position
    Object.defineProperty(this, '__monty_type__', { value: 'FileHandle' })
    Object.freeze(this)
  }

  /** Whether the mode opens the file in binary form. */
  get binary(): boolean {
    return this.mode.includes('b')
  }

  /** Whether the mode permits reads. */
  get readable(): boolean {
    return this.mode.startsWith('r') || this.mode.includes('+')
  }

  /** Whether the mode permits writes. */
  get writable(): boolean {
    return this.mode.startsWith('w') || this.mode.startsWith('a') || this.mode.includes('+')
  }

  /** Treats handles decoded by the native transport as instances too. */
  static [Symbol.hasInstance](value: unknown): boolean {
    return (
      typeof value === 'object' && value !== null && (value as Record<string, unknown>).__monty_type__ === 'FileHandle'
    )
  }
}

/** Canonicalizes the subset of Python file modes Monty supports. */
export function canonicalFileMode(mode: string): string {
  if (mode.length === 0) {
    throw new TypeError('Must have exactly one of create/read/write/append mode and at most one plus')
  }

  let action: string | undefined
  let binary = false
  let text = false
  for (const char of mode) {
    if (char === 'r' || char === 'w' || char === 'a') {
      if (action !== undefined) throw new TypeError('must have exactly one of create/read/write/append mode')
      action = char
    } else if (char === 'x') {
      throw new TypeError('exclusive creation mode is not supported')
    } else if (char === 'b') {
      if (binary) throw new TypeError('invalid mode: binary mode specified twice')
      binary = true
    } else if (char === 't') {
      if (text) throw new TypeError('invalid mode: text mode specified twice')
      text = true
    } else if (char === '+') {
      throw new TypeError("update modes ('+') are not yet supported")
    } else {
      throw new TypeError(`invalid mode: '${char}'`)
    }
  }
  if (binary && text) throw new TypeError("can't have text and binary mode at once")
  if (action === undefined) {
    throw new TypeError('Must have exactly one of create/read/write/append mode and at most one plus')
  }
  return `${action}${binary ? 'b' : ''}`
}

/** Validates that a file position can cross the JavaScript boundary exactly. */
export function validateFilePosition(position: unknown): asserts position is number {
  if (typeof position !== 'number' || !Number.isSafeInteger(position) || position < 0) {
    throw new TypeError('MontyFileHandle position must be a non-negative safe integer')
  }
}
