// Private Node-only hook used by the JavaScript Logfire integration.

import { _installTelemetryAdapter as installNativeAdapter } from '../native-addon.js'

/** Serializable distributed context captured from the active JS OTel context. */
export interface TelemetryParentContext {
  traceId: string
  spanId: string
  traceFlags: number
  traceState?: string
}

/** One timestamp emitted by the native bridge without lossy JS nanoseconds. */
export interface TelemetryTimestamp {
  seconds: string
  nanoseconds: number
}

/** Version-1 native span/log event delivered on the Node event loop. */
export interface TelemetrySpanEvent {
  kind: 'start' | 'end' | 'log' | 'close'
  traceId: string
  spanId?: string
  parentId?: string | null
  traceFlags?: number
  traceState?: string
  name?: string
  level?: string
  timestamp?: TelemetryTimestamp
  status?: 'unset' | 'ok' | 'error'
  statusDescription?: string
  body?: unknown
  attributes?: Record<string, unknown>
  /** A `close` event that discards every span after global bridge disablement. */
  all?: boolean
}

/**
 * One metric measurement, with everything needed to create the instrument
 * named by `name` on first sight.
 *
 * Metrics carry no trace context: they are pool-wide aggregates that cover
 * untraced sessions and worker lifecycle events belonging to no session. An
 * adapter that only reconstructs spans can ignore them.
 */
export interface TelemetryMetricEvent {
  kind: 'metric'
  /** Which instrument records this measurement. */
  metricKind: 'counter' | 'up_down_counter' | 'histogram'
  /** Dotted instrument name, e.g. `monty.pool.checkout.wait`. */
  name: string
  /** UCUM unit: `s`, `By`, `1`, or a `{thing}` annotation for counts. */
  unit: string
  /** One-line description of what the instrument measures. */
  description: string
  value: number
  attributes: Record<string, unknown>
}

/** Version-1 native telemetry event delivered on the Node event loop. */
export type TelemetryEvent = TelemetrySpanEvent | TelemetryMetricEvent

/** Adapter installed by the JavaScript Logfire SDK. */
export interface MontyTelemetryAdapter {
  captureContext(): TelemetryParentContext | undefined
  event(event: TelemetryEvent): void
}

let adapter: MontyTelemetryAdapter | undefined

/** Installs the one-shot Node telemetry adapter. Intended for the Logfire SDK. */
export function _installTelemetryAdapter(version: number, value: MontyTelemetryAdapter): void {
  installNativeAdapter(version, (value) => {
    try {
      adapter?.event(JSON.parse(value) as TelemetryEvent)
    } catch {
      // A broken telemetry adapter must never affect sandbox execution.
    }
  })
  adapter = value
}

/** Captures distributed context synchronously before native `enter`. */
export function captureTelemetryContext(): Record<string, unknown> | undefined {
  if (adapter === undefined) {
    return undefined
  }
  let context: TelemetryParentContext | undefined
  try {
    context = adapter.captureContext()
  } catch {
    adapter = undefined
    return undefined
  }
  return context === undefined
    ? {}
    : {
        traceId: context.traceId,
        spanId: context.spanId,
        traceFlags: context.traceFlags,
        traceState: context.traceState,
      }
}
