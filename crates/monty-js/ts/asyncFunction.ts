// `asyncFunction()`: marks a host function so sandbox calls to it start
// without waiting for the host. Napi-free, so every backend can export it.

/** The own property the native converter (`is_async_marked` in `src/convert.rs`) reads. */
const ASYNC_MARKER = '__monty_async__'

/**
 * Marks a host function so sandbox calls to it start without waiting for the host.
 *
 * A call returns an awaitable at once. A native or remote worker sends the call
 * to the host and keeps running, so `asyncio.gather` over several calls costs one
 * round trip to the worker rather than one per call. The host still runs `fn` as
 * usual and the result arrives at the `await`.
 *
 * The trade-off: errors thrown by `fn`, or a name the host cannot find, surface at
 * the `await` rather than at the call. Use it in `externalLookup` or on an
 * `ExternalModule`'s functions with `feedRun`; `feedStart()` rejects marked calls.
 * The WebAssembly worker suspends at each call instead, with the same results.
 *
 * Returns a wrapper with `fn`'s name; `fn` itself is left unchanged.
 */
export function asyncFunction<F extends (...args: never[]) => unknown>(fn: F): F {
  if (typeof fn !== 'function') {
    throw new TypeError(`asyncFunction() argument must be a function, not ${fn === null ? 'null' : typeof fn}`)
  }
  const wrapper = function (this: unknown, ...args: never[]): unknown {
    return fn.apply(this, args)
  }
  // the name is what a sandbox call resolves the function by
  Object.defineProperty(wrapper, 'name', { value: fn.name })
  markAsync(wrapper)
  return wrapper as unknown as F
}

/** Whether `asyncFunction()` marked `value`. */
export function isAsyncFunction(value: unknown): boolean {
  return (
    typeof value === 'function' &&
    Object.prototype.hasOwnProperty.call(value, ASYNC_MARKER) &&
    (value as unknown as Record<string, unknown>)[ASYNC_MARKER] === true
  )
}

/** Sets the marker on `fn`; used for proxies standing in for a marked function. */
export function markAsync(fn: (...args: never[]) => unknown): void {
  Object.defineProperty(fn, ASYNC_MARKER, { value: true })
}
