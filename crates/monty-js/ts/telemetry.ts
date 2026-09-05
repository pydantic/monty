// Private Node-only hook used by JavaScript OpenTelemetry integrations.

import {
  context,
  createTraceState,
  ROOT_CONTEXT,
  SpanStatusCode,
  trace,
  type Attributes,
  type Context,
  type Counter,
  type Histogram,
  type HrTime,
  type Meter,
  type Span,
  type Tracer,
  type UpDownCounter,
} from '@opentelemetry/api'
import { type LogRecord, type Logger, type SeverityNumber } from '@opentelemetry/api-logs'

import {
  _flushTelemetry as flushNativeTelemetry,
  _installTelemetry as installNativeTelemetry,
} from '../native-addon.js'

/** Standard OpenTelemetry components that receive Monty's host-side telemetry. */
export interface MontyTelemetryComponents {
  tracer?: Tracer
  meter?: Meter
  logger?: Logger
}

/** One timestamp emitted by the native bridge without lossy JS nanoseconds. */
interface TelemetryTimestamp {
  seconds: string
  nanoseconds: number
}

/** Common fields carried by native span and log events. */
interface TelemetryRecord {
  traceId: string
  spanId?: string
  parentId?: string | null
  timestamp?: TelemetryTimestamp
  attributes?: Attributes
}

/** A native event delivered on the Node event loop. */
type TelemetryEvent =
  | (TelemetryRecord & {
      kind: 'start'
      spanId: string
      parentId: string | null
      parentIsRemote: boolean
      traceFlags: number
      traceState: string
      name: string
      timestamp: TelemetryTimestamp
      attributes: Attributes
    })
  | (TelemetryRecord & {
      kind: 'end'
      spanId: string
      timestamp: TelemetryTimestamp
      status: 'unset' | 'ok' | 'error'
      statusDescription?: string
      attributes: Attributes
    })
  | (TelemetryRecord & {
      kind: 'log'
      parentId: string
      severityNumber?: number
      severityText?: string
      timestamp: TelemetryTimestamp
      body?: LogRecord['body']
      attributes: LogRecord['attributes']
    })
  | { kind: 'close'; traceId: string; spanId: string; all: boolean }
  | { kind: 'flush' }

/** One raw metric measurement delivered by the native bridge. */
type MetricEvent =
  | {
      kind: 'metric'
      instrumentKind: 'counter' | 'upDownCounter' | 'histogram'
      name: string
      unit: string
      description: string
      value: number
      attributes: Attributes
    }
  | { kind: 'flush' }

/** Synchronous host metric instrument created from a native definition. */
type MetricInstrument = Counter | UpDownCounter | Histogram

/** Host span retained until the corresponding Rust span ends. */
interface SpanState {
  span: Span
  context: Context
  root: string
  initialAttributes: Attributes
}

/** Process-global components and their reconstructed host-side state. */
interface InstalledTelemetry extends MontyTelemetryComponents {
  spans: Map<string, SpanState>
  instruments: Map<string, MetricInstrument>
}

let installed: InstalledTelemetry | undefined

/** Installs process-wide telemetry using standard OpenTelemetry components. */
export function _installTelemetry(components: MontyTelemetryComponents): void {
  if (components.tracer === undefined && components.meter === undefined && components.logger === undefined) {
    throw new TypeError('at least one OpenTelemetry component is required')
  }
  if (installed !== undefined) {
    throw new Error('Monty telemetry is already configured')
  }

  const state: InstalledTelemetry = {
    ...components,
    spans: new Map(),
    instruments: new Map(),
  }
  installNativeTelemetry(
    (value) => receiveTelemetryEvent(state, value),
    (value) => receiveMetricEvent(state, value),
  )
  installed = state
}

/** Flushes native callback queues before the host SDK is flushed or shut down. */
export async function _flushTelemetry(): Promise<void> {
  await flushNativeTelemetry()
}

/** Captures distributed context synchronously before native `enter`. */
export function captureTelemetryContext(): Record<string, unknown> | undefined {
  if (installed?.tracer === undefined && installed?.logger === undefined) {
    return undefined
  }
  const spanContext = trace.getSpanContext(context.active())
  return spanContext === undefined || !trace.isSpanContextValid(spanContext)
    ? {}
    : {
        traceId: spanContext.traceId,
        spanId: spanContext.spanId,
        traceFlags: spanContext.traceFlags,
        traceState: spanContext.traceState?.serialize(),
      }
}

/** Delivers one span, log, cleanup, or flush event without affecting Monty on failure. */
function receiveTelemetryEvent(state: InstalledTelemetry, value: string): boolean {
  let event: TelemetryEvent
  try {
    event = JSON.parse(value) as TelemetryEvent
  } catch {
    state.tracer = undefined
    state.logger = undefined
    state.spans.clear()
    return false
  }

  switch (event.kind) {
    case 'start':
      return startSpan(state, event)
    case 'end':
      return endSpan(state, event)
    case 'log':
      return emitLog(state, event)
    case 'close':
      closeSpans(state, event)
      return true
    case 'flush':
      return true
  }
}

/** Reconstructs a host span while preserving non-recording spans as parents. */
function startSpan(state: InstalledTelemetry, event: Extract<TelemetryEvent, { kind: 'start' }>): boolean {
  const tracer = state.tracer
  if (tracer === undefined) {
    return true
  }
  try {
    const key = spanKey(event.traceId, event.spanId)
    const parentKey = event.parentId === null ? undefined : spanKey(event.traceId, event.parentId)
    const parent = parentKey === undefined ? undefined : state.spans.get(parentKey)
    const parentContext = parent?.context ?? externalParentContext(event)
    const span = tracer.startSpan(
      event.name,
      { attributes: event.attributes, startTime: timestamp(event.timestamp) },
      parentContext,
    )
    state.spans.set(key, {
      span,
      context: trace.setSpan(parentContext, span),
      root: parent?.root ?? key,
      initialAttributes: event.attributes,
    })
    return true
  } catch {
    state.tracer = undefined
    state.spans.clear()
    return true
  }
}

/** Applies final span data and ends the corresponding host span. */
function endSpan(state: InstalledTelemetry, event: Extract<TelemetryEvent, { kind: 'end' }>): boolean {
  if (state.tracer === undefined) {
    return true
  }
  const key = spanKey(event.traceId, event.spanId)
  const spanState = state.spans.get(key)
  if (spanState === undefined) {
    return false
  }
  state.spans.delete(key)
  try {
    const attributes = changedAttributes(event.attributes, spanState.initialAttributes)
    if (Object.keys(attributes).length !== 0) {
      spanState.span.setAttributes(attributes)
    }
    if (event.status === 'ok') {
      spanState.span.setStatus({ code: SpanStatusCode.OK })
    } else if (event.status === 'error') {
      spanState.span.setStatus({ code: SpanStatusCode.ERROR, message: event.statusDescription })
    }
    spanState.span.end(timestamp(event.timestamp))
    return true
  } catch {
    state.tracer = undefined
    state.spans.clear()
    return true
  }
}

/** Emits one log through the supplied logger under its reconstructed parent. */
function emitLog(state: InstalledTelemetry, event: Extract<TelemetryEvent, { kind: 'log' }>): boolean {
  const logger = state.logger
  if (logger === undefined) {
    return true
  }
  try {
    const parent = state.tracer === undefined ? undefined : state.spans.get(spanKey(event.traceId, event.parentId))
    logger.emit({
      context: parent?.context ?? ROOT_CONTEXT,
      timestamp: timestamp(event.timestamp),
      severityNumber: event.severityNumber as SeverityNumber | undefined,
      severityText: event.severityText,
      body: event.body,
      attributes: event.attributes,
    })
  } catch {
    state.logger = undefined
  }
  return true
}

/** Removes spans belonging to one failed root, or every span after global failure. */
function closeSpans(state: InstalledTelemetry, event: Extract<TelemetryEvent, { kind: 'close' }>): void {
  if (event.all) {
    state.spans.clear()
  } else {
    const root = spanKey(event.traceId, event.spanId)
    for (const [key, spanState] of state.spans) {
      if (spanState.root === root) {
        state.spans.delete(key)
      }
    }
  }
}

/** Records one raw measurement through a lazily created host instrument. */
function receiveMetricEvent(state: InstalledTelemetry, value: string): boolean {
  const meter = state.meter
  if (meter === undefined) {
    return true
  }
  try {
    const event = JSON.parse(value) as MetricEvent
    if (event.kind === 'flush') {
      return true
    }
    let instrument = state.instruments.get(event.name)
    if (instrument === undefined) {
      const options = { unit: event.unit, description: event.description }
      switch (event.instrumentKind) {
        case 'counter':
          instrument = meter.createCounter(event.name, options)
          break
        case 'upDownCounter':
          instrument = meter.createUpDownCounter(event.name, options)
          break
        case 'histogram':
          instrument = meter.createHistogram(event.name, options)
          break
      }
      state.instruments.set(event.name, instrument)
    }
    if (event.instrumentKind === 'histogram') {
      ;(instrument as Histogram).record(event.value, event.attributes, ROOT_CONTEXT)
    } else {
      ;(instrument as Counter | UpDownCounter).add(event.value, event.attributes, ROOT_CONTEXT)
    }
  } catch {
    state.meter = undefined
    state.instruments.clear()
  }
  return true
}

/** Reconstructs an external parent from the propagated native span context. */
function externalParentContext(event: Extract<TelemetryEvent, { kind: 'start' }>): Context {
  if (event.parentId === null) {
    return ROOT_CONTEXT
  }
  return trace.setSpanContext(ROOT_CONTEXT, {
    traceId: event.traceId,
    spanId: event.parentId,
    traceFlags: event.traceFlags,
    traceState: event.traceState === '' ? undefined : createTraceState(event.traceState),
    isRemote: event.parentIsRemote,
  })
}

/** Converts the lossless wire timestamp to OpenTelemetry's tuple representation. */
function timestamp(value: TelemetryTimestamp): HrTime {
  return [Number(value.seconds), value.nanoseconds]
}

/** Identifies a native span without assuming span IDs are process-global. */
function spanKey(traceId: string, spanId: string): string {
  return `${traceId}:${spanId}`
}

/** Returns attributes added or changed after span creation. */
function changedAttributes(attributes: Attributes, initial: Attributes): Attributes {
  return Object.fromEntries(
    Object.entries(attributes).filter(([key, value]) => JSON.stringify(initial[key]) !== JSON.stringify(value)),
  )
}
