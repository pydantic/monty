declare const MONTY_TEST_WASM: boolean

/** Selects capabilities independently of whether the test runner itself is Node. */
export const isWasm = typeof window !== 'undefined' || (typeof MONTY_TEST_WASM !== 'undefined' && MONTY_TEST_WASM)
export const kind = typeof window === 'undefined' ? 'node' : 'browser'

interface SkipContext {
  skip(): void
}

/** OS-process and host-filesystem assertions apply only to the native backend. */
export function skipIfWasm(ctx: SkipContext): void {
  if (isWasm) ctx.skip()
}

/** Unsupported-capability assertions apply only to WASM. */
export function skipIfNative(ctx: SkipContext): void {
  if (!isWasm) ctx.skip()
}
