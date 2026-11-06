declare const MONTY_TEST_WASM: boolean

/** Selects capabilities independently of whether the test runner itself is Node. */
export const isWasm = typeof window !== 'undefined' || (typeof MONTY_TEST_WASM !== 'undefined' && MONTY_TEST_WASM)
/** CPU execution clocks are supported only by native workers on Linux. */
export const supportsCpuClock = !isWasm && typeof process !== 'undefined' && process.platform === 'linux'
export const kind = typeof window !== 'undefined' ? 'browser WASM' : isWasm ? 'Node WASM' : 'native Node'

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
