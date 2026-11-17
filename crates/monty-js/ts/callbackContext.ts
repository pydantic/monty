// Node supplies suspension spans; browser transports preserve the captured context.

import { diag, trace, type Context, type Span } from '@opentelemetry/api'

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
