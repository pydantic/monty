// Checkout-option normalization shared by the napi binding (`pool.ts`) and
// the wasm worker transport (`worker/transport.ts`), which encode the same
// wire protocol but through different layers. Must stay napi-free so the
// wasm bundle can import it.

/** ty's diagnostic renderings, as accepted by the `typeCheckFormat` option. */
export type TypeCheckFormat =
  | 'full'
  | 'concise'
  | 'azure'
  | 'json'
  | 'jsonlines'
  | 'rdjson'
  | 'pylint'
  | 'gitlab'
  | 'github'

/**
 * `Configure.type_check_format` wire numbers (see
 * proto/monty/v1/monty.proto); 0 is UNSPECIFIED, which the child renders as
 * `full`. Only the wasm transport needs these — the napi binding passes the
 * name through to Rust.
 */
const TYPE_CHECK_FORMATS: Record<TypeCheckFormat, number> = {
  full: 1,
  concise: 2,
  azure: 3,
  json: 4,
  jsonlines: 5,
  rdjson: 6,
  pylint: 7,
  gitlab: 8,
  github: 9,
}

/**
 * Encodes a {@link TypeCheckFormat} to its wire number, throwing on an unknown name.
 *
 * The own-property check matters: JavaScript callers are not bound by the
 * type, and a plain lookup would find inherited names like `'toString'` and
 * hand a `Function` to the wire encoder instead of rejecting it here.
 */
export function encodeTypeCheckFormat(format: TypeCheckFormat): number {
  const encoded = Object.hasOwn(TYPE_CHECK_FORMATS, format) ? TYPE_CHECK_FORMATS[format] : undefined
  if (encoded === undefined) {
    throw new RangeError(
      `unknown typeCheckFormat '${format}', expected one of: ${Object.keys(TYPE_CHECK_FORMATS).join(', ')}`,
    )
  }
  return encoded
}

/**
 * The `assertMessageAnnotations` checkout option: `true`/`false`, or an
 * integer customizing the per-operand repr truncation length (in bytes,
 * default 120) of introspected `assert` failure messages.
 */
export type AssertMessageAnnotations = boolean | number

/**
 * Normalizes {@link AssertMessageAnnotations} to the wire encoding of
 * `Configure.assert_message_annotations`: `undefined`/`true` → absent (the
 * child's default, a 120-byte truncation), `false` → `0` (off), an integer →
 * a custom truncation length. Throws `RangeError` for numbers the wire's
 * uint32 cannot carry (non-integers, `< 1`, `> 2**32 - 1`).
 */
export function encodeAssertMessageAnnotations(value: AssertMessageAnnotations | undefined): number | undefined {
  if (value === undefined || value === true) return undefined
  if (value === false) return 0
  if (!Number.isInteger(value) || value < 1 || value > 0xffff_ffff) {
    throw new RangeError('assertMessageAnnotations must be a boolean or an integer between 1 and 2**32 - 1')
  }
  return value
}

/**
 * Clock for `date.today()`, `datetime.now()` and `time.time()`; defaults to the worker's clock.
 * `'call_host'` delegates to `os`; a `Date` freezes the instant and defaults `timezone` to UTC.
 */
export type DateTimeSource = 'system' | 'call_host' | Date

/**
 * The sandbox's local zone, read by naive `datetime.now()` and `date.today()`, `astimezone()`,
 * `time.timezone`/`time.tzname` and `%Z`; defaults to `'utc'`, never the worker's own zone.
 * `'call_host'` delegates calls requiring the zone to `os`; an object supplies a fixed UTC offset
 * and optional name, as in `datetime.timezone`. IANA zones are unsupported.
 */
export type TimeZone = 'utc' | 'call_host' | { offsetSeconds: number; name?: string }

/**
 * Sleep policy: `'system'` (default) waits in the pool, capped per call by `sleepSystemMax`;
 * `'call_host'` delegates to `os`; `'zero'` returns immediately.
 */
export type SleepMode = 'system' | 'call_host' | 'zero'

/**
 * Initial `random` state: `'system'` (default) uses worker OS entropy; `'call_host'` requests
 * 2496 bytes from `os.urandom` via `os` on the first draw. `{ seed }` applies `random.seed(seed)`:
 * integral numbers and bigints become ints, other finite numbers become floats, strings become str,
 * and Uint8Array becomes bytes.
 */
export type RandomStart = 'system' | 'call_host' | { seed: number | bigint | string | Uint8Array }

/**
 * Session clock, sleep and random initialization policies. Omitted fields retain their defaults.
 */
export interface AutoOsCalls {
  datetime?: DateTimeSource
  timezone?: TimeZone
  sleep?: SleepMode
  /**
   * Maximum seconds per `'system'` sleep (default 10; `Infinity` disables the cap).
   * Raises `RangeError` with other sleep modes.
   */
  sleepSystemMax?: number
  randomStart?: RandomStart
}

/** A frozen clock reading, as the wire carries it. */
export interface FixedDateTime {
  unixSeconds: bigint
  microsecond: number
}

/** A fixed zone, as the wire carries it. */
export interface FixedTimeZone {
  offsetSeconds: number
  name?: string
}

/** A `random.seed()` argument in its wire form: one of the four CPython types. */
export type EncodedRandomSeed = { int: Uint8Array } | { float: number } | { str: string } | { bytes: Uint8Array }

/**
 * The options normalized to their wire shapes, shared by the napi binding
 * and the wasm transport. An absent field is the worker's default.
 */
export interface EncodedAutoOsCalls {
  datetime?: 'system' | 'call_host' | FixedDateTime
  timezone?: 'utc' | 'call_host' | FixedTimeZone
  sleep?: SleepMode
  /** Seconds; `Infinity` lifts the cap. */
  sleepSystemMaxSecs?: number
  /** Absent means `'system'`. */
  randomStart?: 'call_host' | { seed: EncodedRandomSeed }
}

const SLEEP_MODES: readonly SleepMode[] = ['system', 'call_host', 'zero']

/** Mirrors monty-types' `SleepMode::DEFAULT_MAX`: the cap on one `'system'` sleep, in seconds. */
const DEFAULT_SLEEP_SYSTEM_MAX_SECS = 10

/** The widest fixed zone `datetime.timezone` accepts: strictly within a day of UTC. */
const MAX_TIMEZONE_OFFSET_SECONDS = 86_399

/** The longest duration the wire's `u64` microseconds can carry, in whole seconds. */
const MAX_WIRE_SECS = 18_446_744_073_709

/**
 * Host-enforced cap in seconds, applied even when a restored dump requests system sleeps.
 * The worker's own cap cannot be trusted at this boundary.
 */
export function systemSleepCapOf(calls: EncodedAutoOsCalls): number {
  return calls.sleepSystemMaxSecs ?? DEFAULT_SLEEP_SYSTEM_MAX_SECS
}

/**
 * Validates options at runtime before wire encoding; callers need not obey TypeScript types.
 */
export function encodeAutoOsCalls(options: AutoOsCalls): EncodedAutoOsCalls {
  const encoded: EncodedAutoOsCalls = {}
  if (options.datetime !== undefined) {
    encoded.datetime = encodeDateTime(options.datetime)
    // a Date is read in the UTC default unless the zone is given explicitly
    if (options.datetime instanceof Date) encoded.timezone = { offsetSeconds: 0, name: 'UTC' }
  }
  if (options.timezone !== undefined) {
    encoded.timezone = encodeTimeZone(options.timezone)
  }
  if (options.sleep !== undefined) {
    if (!SLEEP_MODES.includes(options.sleep)) {
      throw new RangeError(`unknown sleep '${String(options.sleep)}', expected one of: ${SLEEP_MODES.join(', ')}`)
    }
    encoded.sleep = options.sleep
  }
  if (options.sleepSystemMax !== undefined) {
    const secs = options.sleepSystemMax
    if (typeof secs !== 'number' || Number.isNaN(secs) || secs < 0) {
      throw new RangeError('sleepSystemMax must be a non-negative number of seconds (Infinity for no cap)')
    }
    // a finite cap must fit the wire's u64 microseconds; Infinity is the no-cap sentinel
    if (secs !== Infinity && secs > MAX_WIRE_SECS) {
      throw new RangeError(`sleepSystemMax must be at most ${MAX_WIRE_SECS} seconds (Infinity for no cap)`)
    }
    if (options.sleep !== undefined && options.sleep !== 'system') {
      throw new RangeError(`sleepSystemMax only applies to sleep: 'system', not '${options.sleep}'`)
    }
    encoded.sleepSystemMaxSecs = secs
  }
  if (options.randomStart === 'call_host') {
    encoded.randomStart = 'call_host'
  } else if (options.randomStart !== undefined && options.randomStart !== 'system') {
    encoded.randomStart = { seed: encodeRandomSeed(options.randomStart) }
  }
  return encoded
}

function encodeDateTime(datetime: DateTimeSource): 'system' | 'call_host' | FixedDateTime {
  if (datetime === 'system' || datetime === 'call_host') return datetime
  if (!(datetime instanceof Date) || Number.isNaN(datetime.getTime())) {
    throw new RangeError("datetime must be 'system', 'call_host' or a valid Date")
  }
  const ms = datetime.getTime()
  const seconds = Math.floor(ms / 1000)
  return { unixSeconds: BigInt(seconds), microsecond: (ms - seconds * 1000) * 1000 }
}

function encodeTimeZone(timezone: TimeZone): 'utc' | 'call_host' | FixedTimeZone {
  if (timezone === 'utc' || timezone === 'call_host') return timezone
  const shape = "timezone must be 'utc', 'call_host' or { offsetSeconds: number, name?: string }"
  if (typeof timezone !== 'object' || timezone === null || !Object.hasOwn(timezone, 'offsetSeconds')) {
    throw new TypeError(shape)
  }
  const { offsetSeconds, name } = timezone
  if (!Number.isInteger(offsetSeconds)) {
    throw new RangeError('timezone offsetSeconds must be an integer number of seconds')
  }
  if (Math.abs(offsetSeconds) > MAX_TIMEZONE_OFFSET_SECONDS) {
    throw new RangeError(`timezone offsetSeconds must be within ±${MAX_TIMEZONE_OFFSET_SECONDS}, got ${offsetSeconds}`)
  }
  if (name !== undefined && typeof name !== 'string') {
    throw new TypeError('timezone name must be a string')
  }
  return name === undefined ? { offsetSeconds } : { offsetSeconds, name }
}

function encodeRandomSeed(start: RandomStart): EncodedRandomSeed {
  const seed = typeof start === 'object' && start !== null && Object.hasOwn(start, 'seed') ? start.seed : undefined
  if (typeof seed === 'bigint') return { int: bigintToSignedLeBytes(seed) }
  if (typeof seed === 'number') {
    // Reject non-finite seeds before sending them to the wire decoder.
    if (!Number.isFinite(seed)) throw new RangeError(`randomStart seed must be finite, got ${seed}`)
    return Number.isInteger(seed) ? { int: bigintToSignedLeBytes(BigInt(seed)) } : { float: seed }
  }
  if (typeof seed === 'string') return { str: seed }
  if (seed instanceof Uint8Array) return { bytes: seed }
  throw new TypeError("randomStart must be 'system', 'call_host' or { seed: number | bigint | string | Uint8Array }")
}

/** Two's-complement little-endian bytes of `n`, as `BigInt::from_signed_bytes_le` reads them. */
function bigintToSignedLeBytes(n: bigint): Uint8Array {
  const bytes: number[] = []
  let remaining = n
  // Emit bytes until the remaining value is the sign extension of the last byte.
  for (;;) {
    const byte = Number(remaining & 0xffn)
    bytes.push(byte)
    remaining >>= 8n
    const done = remaining === 0n && byte < 0x80
    const doneNegative = remaining === -1n && byte >= 0x80
    if (done || doneNegative) break
  }
  return Uint8Array.from(bytes)
}
