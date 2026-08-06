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

/** Encodes a {@link TypeCheckFormat} to its wire number, throwing on an unknown name. */
export function encodeTypeCheckFormat(format: TypeCheckFormat): number {
  const encoded = TYPE_CHECK_FORMATS[format]
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
