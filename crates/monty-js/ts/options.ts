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
 * `AutoOsCalls.datetime`: the instant `date.today()`, `datetime.now()` and
 * `time.time()` read. `'system'` (the default) is the worker's clock,
 * `'call_host'` sends each call to the `os` callback, and a `Date` freezes
 * the clock at that instant — and, unless `timezone` is given, sets the zone
 * to UTC, so `datetime.now()` returns it exactly.
 */
export type DateTimeSource = 'system' | 'call_host' | Date

/**
 * `AutoOsCalls.timezone`: the local zone naive `datetime.now()` and
 * `date.today()` read in. `'system'` (the default) is the worker's local
 * zone, `'call_host'` sends the calls that need the zone to the `os`
 * callback, and an object is a fixed offset from UTC with an optional name —
 * what `datetime.timezone(offset, name)` carries, not an IANA zone.
 */
export type TimeZone = 'system' | 'call_host' | { offsetSeconds: number; name?: string }

/**
 * `AutoOsCalls.sleep`: what `time.sleep()` and `asyncio.sleep()` do.
 * `'system'` (the default) has this process wait, each call cut to
 * `sleepSystemMax`, without consulting the `os` callback; `'call_host'`
 * sends both to the `os` callback; `'zero'` returns at once.
 */
export type SleepMode = 'system' | 'call_host' | 'zero'

/**
 * `AutoOsCalls.randomStart`: where an unseeded `random` generator gets its
 * first state. `'system'` (the default) is the worker's OS entropy;
 * `'call_host'` sends an `os.urandom` request for 2496 bytes to the `os`
 * callback on the first draw; `{ seed }` starts it as `random.seed(seed)`
 * would, with the types CPython accepts (a `number` is an int when integral,
 * `bigint` for larger ints, `string`, or `Uint8Array` for `bytes`).
 */
export type RandomStart = 'system' | 'call_host' | { seed: number | bigint | string | Uint8Array }

/**
 * The `autoOsCalls` checkout option: which OS calls the worker answers
 * itself, for the life of the session. Every field is optional; an omitted
 * one keeps its default.
 */
export interface AutoOsCalls {
  datetime?: DateTimeSource
  timezone?: TimeZone
  sleep?: SleepMode
  /**
   * Longest wait a `'system'` sleep performs per call, in seconds (default
   * 10; `Infinity` for no cap). Given alongside any other `sleep` it is a
   * `RangeError`, not ignored.
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
  timezone?: 'system' | 'call_host' | FixedTimeZone
  sleep?: SleepMode
  /** Seconds; `Infinity` lifts the cap. */
  sleepSystemMaxSecs?: number
  /** Absent means `'system'`. */
  randomStart?: 'call_host' | { seed: EncodedRandomSeed }
}

const SLEEP_MODES: readonly SleepMode[] = ['system', 'call_host', 'zero']

/**
 * The sleeps this process waits out itself, without the `os` callback:
 * `sleep: 'system'` (the default) with its cap in seconds. `null` when the
 * `os` callback (`'call_host'`) or nothing (`'zero'`) answers them.
 */
export interface SystemSleep {
  readonly maxSecs: number
}

/** The sleep policy the encoded options imply for the session's host; see `SystemSleep`. */
export function systemSleepOf(calls: EncodedAutoOsCalls): SystemSleep | null {
  if (calls.sleep !== undefined && calls.sleep !== 'system') return null
  return { maxSecs: calls.sleepSystemMaxSecs ?? 10 }
}

/**
 * Validates and normalizes the options, throwing `RangeError` / `TypeError`
 * for a value the wire cannot carry. Own-property and instance checks matter
 * here as in {@link encodeTypeCheckFormat}: callers are not bound by the
 * types.
 */
export function encodeAutoOsCalls(options: AutoOsCalls): EncodedAutoOsCalls {
  const encoded: EncodedAutoOsCalls = {}
  if (options.datetime !== undefined) {
    encoded.datetime = encodeDateTime(options.datetime)
    // a Date is read as UTC unless the zone is given explicitly
    if (options.datetime instanceof Date) encoded.timezone = { offsetSeconds: 0 }
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
    // a cap on a sleep that never happens in the worker is a contradiction, not something to ignore
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

/** A `Date` becomes its instant; the two names pass through. */
function encodeDateTime(datetime: DateTimeSource): 'system' | 'call_host' | FixedDateTime {
  if (datetime === 'system' || datetime === 'call_host') return datetime
  if (!(datetime instanceof Date) || Number.isNaN(datetime.getTime())) {
    throw new RangeError("datetime must be 'system', 'call_host' or a valid Date")
  }
  const ms = datetime.getTime()
  const seconds = Math.floor(ms / 1000)
  return { unixSeconds: BigInt(seconds), microsecond: (ms - seconds * 1000) * 1000 }
}

/** A fixed zone is validated field by field; the two names pass through. */
function encodeTimeZone(timezone: TimeZone): 'system' | 'call_host' | FixedTimeZone {
  if (timezone === 'system' || timezone === 'call_host') return timezone
  const shape = "timezone must be 'system', 'call_host' or { offsetSeconds: number, name?: string }"
  if (typeof timezone !== 'object' || timezone === null || !Object.hasOwn(timezone, 'offsetSeconds')) {
    throw new TypeError(shape)
  }
  const { offsetSeconds, name } = timezone
  if (!Number.isInteger(offsetSeconds) || Math.abs(offsetSeconds) > 0x7fff_ffff) {
    throw new RangeError('timezone offsetSeconds must be an integer number of seconds')
  }
  if (name !== undefined && typeof name !== 'string') {
    throw new TypeError('timezone name must be a string')
  }
  return name === undefined ? { offsetSeconds } : { offsetSeconds, name }
}

/** The seed in its wire form; `bool` and other types are refused. */
function encodeRandomSeed(start: RandomStart): EncodedRandomSeed {
  const seed = typeof start === 'object' && start !== null && Object.hasOwn(start, 'seed') ? start.seed : undefined
  if (typeof seed === 'bigint') return { int: bigintToSignedLeBytes(seed) }
  if (typeof seed === 'number') {
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
