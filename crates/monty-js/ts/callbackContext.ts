// Node supplies suspension spans; browser transports preserve the captured context.

import { context, diag, trace, type Context, type Span } from '@opentelemetry/api'

type NativePrintCallback = (stream: 'stdout' | 'stderr', text: string, parent?: string | null) => void
/** Announces a call the worker streamed mid-turn: its call ID and its span key. */
type NativeStreamedCallCallback = (callId: number, parent?: string | null) => void

interface CallbackContextHandlers {
  bindPrint(callback: NativePrintCallback): NativePrintCallback
  bindStreamedCall(callback: NativeStreamedCallCallback): NativeStreamedCallCallback
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

/** Binds a streamed-call callback to the caller's async context, run under the call's span. */
export function bindStreamedCallCallback(callback: NativeStreamedCallCallback): NativeStreamedCallCallback {
  return handlers?.bindStreamedCall(callback) ?? callback
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

/**
 * The W3C `traceparent` of the span active where a feed or load was called, sent
 * with the request so a server recording the session parents the turn's spans to it.
 */
export function captureTraceparent(): string | undefined {
  try {
    const span = trace.getSpanContext(context.active())
    if (span === undefined || !trace.isSpanContextValid(span)) {
      return undefined
    }
    return `00-${span.traceId}-${span.spanId}-${span.traceFlags.toString(16).padStart(2, '0')}`
  } catch {
    return undefined
  }
}

const TRACEPARENT = /^00-([0-9a-f]{32})-([0-9a-f]{16})-([0-9a-f]{2})$/

/** The span a server named for a suspension, as a non-recording span to parent host work to. */
export function remoteCallbackSpan(parent: string): Span | undefined {
  const match = TRACEPARENT.exec(parent)
  if (match === null) {
    return undefined
  }
  const [, traceId, spanId, flags] = match as unknown as [string, string, string, string]
  return trace.wrapSpanContext({ traceId, spanId, traceFlags: parseInt(flags, 16), isRemote: true })
}
