// Node supplies suspension spans; browser transports preserve the captured context.

import { diag, trace, type Context, type Span } from '@opentelemetry/api'

type NativePrintCallback = (stream: 'stdout' | 'stderr', text: string, parent?: string | null) => void

/** The native async-call sink: an opaque call handle and the call span's key. */
type NativeAsyncCallCallback = (raw: unknown, parent?: string | null) => Promise<void>

interface CallbackContextHandlers {
  bindPrint(callback: NativePrintCallback): NativePrintCallback
  bindAsyncCall(callback: (raw: unknown) => Promise<void>): NativeAsyncCallCallback
  run<T>(parent: string | undefined, callback: () => T): T
  span(parent: string | undefined): Span | undefined
}

let handlers: CallbackContextHandlers | undefined

export function setCallbackContextHandlers(value: CallbackContextHandlers): void {
  handlers = value
}

export function bindPrintCallback(callback: NativePrintCallback): NativePrintCallback {
  return handlers?.bindPrint(callback) ?? callback
}

/**
 * Binds the sink for streamed async calls to the feed caller's async context
 * and, under tracing, to each call's span: the host function then runs
 * where a suspended call's would.
 */
export function bindAsyncCallCallback(callback: (raw: unknown) => Promise<void>): NativeAsyncCallCallback {
  return handlers?.bindAsyncCall(callback) ?? ((raw) => callback(raw))
}

/** Returns the suspension's span in its captured context, preserving baggage and other entries. */
export function getCallbackContext(parent: string | undefined, captured: Context): Context {
  try {
    const span = handlers?.span(parent)
    return span === undefined ? captured : trace.setSpan(captured, span)
  } catch (error) {
    diag.warn('Monty could not compose the snapshot trace context; using the captured context', error)
    return captured
  }
}

export function runWithCallbackContext<T>(parent: string | undefined, callback: () => T): T {
  return handlers === undefined ? callback() : handlers.run(parent, callback)
}
