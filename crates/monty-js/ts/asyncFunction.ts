// Detects the host functions whose sandbox calls start without waiting for the host.

/**
 * Whether `value` is an `async function`, read from its `Object.prototype.toString`
 * tag as the native converter (`is_async_function` in `src/convert.rs`) does.
 * Bound and proxied async functions keep the tag; a transpiled one, which is a
 * plain function returning a promise, does not.
 */
export function isAsyncFunction(value: unknown): boolean {
  return typeof value === 'function' && Object.prototype.toString.call(value) === '[object AsyncFunction]'
}
