// Checkout-option normalization shared by the napi binding (`pool.ts`) and
// the wasm worker transport (`worker/transport.ts`), which encode the same
// wire protocol but through different layers. Must stay napi-free so the
// wasm bundle can import it.

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
