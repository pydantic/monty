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
 * The `datetime` checkout option: what `date.today()`, `datetime.now()` and
 * `time.time()` read. `'system'` (the default) is the worker's clock and local
 * timezone, `'call_host'` sends each call to the `os` callback, and a `Date`
 * freezes the clock at that instant (read as UTC).
 */
export type DateTimeSource = 'call_host' | 'system' | Date

/**
 * The `sleep` checkout option: what `time.sleep()` and `asyncio.sleep()` do.
 * `'sandbox_sleep'` (the default) waits inside the worker, `'zero'` returns
 * at once, `'call_host'` sends both to the `os` callback.
 */
export type SleepMode = 'call_host' | 'zero' | 'sandbox_sleep'

/**
 * The `randomStart` checkout option: where an unseeded `random` generator
 * gets its first state. `'random'` (the default) is the worker's OS entropy;
 * `{ seed }` starts it as `random.seed(seed)` would, with the types CPython
 * accepts (a `number` is an int when integral, `bigint` for larger ints,
 * `string`, or `Uint8Array` for `bytes`).
 */
export type RandomStart = 'random' | { seed: number | bigint | string | Uint8Array }

/** The four options as they appear on `CheckoutOptions`. */
export interface AutoOsCallsOptions {
  datetime?: DateTimeSource
  sleep?: SleepMode
  sandboxSleepClamp?: number
  randomStart?: RandomStart
}

/** A frozen clock reading, as the wire carries it. */
export interface FixedDateTime {
  unixSeconds: bigint
  microsecond: number
  localOffsetSeconds: number
}

/** A `random.seed()` argument in its wire form: one of the four CPython types. */
export type EncodedRandomSeed = { int: Uint8Array } | { float: number } | { str: string } | { bytes: Uint8Array }

/**
 * The options normalized to their wire shapes, shared by the napi binding
 * and the wasm transport. An absent field is the worker's default.
 */
export interface EncodedAutoOsCalls {
  datetime?: 'call_host' | 'system' | FixedDateTime
  sleep?: SleepMode
  /** Seconds; `Infinity` lifts the cap. */
  sandboxSleepClampSecs?: number
  /** Absent means `'random'`. */
  randomSeed?: EncodedRandomSeed
}

const SLEEP_MODES: readonly SleepMode[] = ['call_host', 'zero', 'sandbox_sleep']

/**
 * Validates and normalizes the four options, throwing `RangeError` /
 * `TypeError` for a value the wire cannot carry. Own-property and instance
 * checks matter here as in {@link encodeTypeCheckFormat}: callers are not
 * bound by the types.
 */
export function encodeAutoOsCalls(options: AutoOsCallsOptions): EncodedAutoOsCalls {
  const encoded: EncodedAutoOsCalls = {}
  if (options.datetime !== undefined) {
    encoded.datetime = encodeDateTime(options.datetime)
  }
  if (options.sleep !== undefined) {
    if (!SLEEP_MODES.includes(options.sleep)) {
      throw new RangeError(`unknown sleep '${String(options.sleep)}', expected one of: ${SLEEP_MODES.join(', ')}`)
    }
    encoded.sleep = options.sleep
  }
  if (options.sandboxSleepClamp !== undefined) {
    const secs = options.sandboxSleepClamp
    if (typeof secs !== 'number' || Number.isNaN(secs) || secs < 0) {
      throw new RangeError('sandboxSleepClamp must be a non-negative number of seconds (Infinity for no cap)')
    }
    encoded.sandboxSleepClampSecs = secs
  }
  if (options.randomStart !== undefined && options.randomStart !== 'random') {
    encoded.randomSeed = encodeRandomSeed(options.randomStart)
  }
  return encoded
}

/** A `Date` becomes its instant read as UTC; the two names pass through. */
function encodeDateTime(datetime: DateTimeSource): 'call_host' | 'system' | FixedDateTime {
  if (datetime === 'call_host' || datetime === 'system') return datetime
  if (!(datetime instanceof Date) || Number.isNaN(datetime.getTime())) {
    throw new RangeError("datetime must be 'system', 'call_host' or a valid Date")
  }
  const ms = datetime.getTime()
  const seconds = Math.floor(ms / 1000)
  return { unixSeconds: BigInt(seconds), microsecond: (ms - seconds * 1000) * 1000, localOffsetSeconds: 0 }
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
  throw new TypeError("randomStart must be 'random' or { seed: number | bigint | string | Uint8Array }")
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
