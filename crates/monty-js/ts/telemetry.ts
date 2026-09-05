import type {
  Attributes,
  Context,
  Counter,
  Histogram,
  Meter,
  MeterProvider,
  Span,
  Tracer,
  TracerProvider,
  UpDownCounter,
} from '@opentelemetry/api'
import {
  ROOT_CONTEXT,
  SpanStatusCode,
  createTraceState,
  context as ContextAPI,
  metrics as MetricsAPI,
  trace as TraceAPI,
} from '@opentelemetry/api'
import type { AnyValue, AnyValueMap, Logger, LogRecord, SeverityNumber } from '@opentelemetry/api-logs'
import { logs as LogsAPI } from '@opentelemetry/api-logs'

import {
  _flushTelemetry as flushNativeTelemetry,
  _installTelemetry as installNativeTelemetry,
  _montyVersion as montyVersion,
  _setTelemetryMetricsEnabled as setNativeMetricsEnabled,
} from '../native-addon.js'

const OTEL_SCOPE = '@pydantic/monty'
const OTEL_VERSION = montyVersion()

/** Standard OpenTelemetry components used to record Monty telemetry. */
export interface TelemetryComponents {
  tracer?: Tracer
  meter?: Meter
  logger?: Logger
}

/** Configuration for [`MontyInstrumentation`]. */
export interface MontyInstrumentationConfig {
  enabled?: boolean
  traces?: boolean
  metrics?: boolean
  logs?: boolean
}

interface TelemetryTimestamp {
  seconds: string
  nanoseconds: number
}

interface StartEvent {
  kind: 'start'
  traceId: string
  spanId: string
  parentId: string | null
  parentIsRemote: boolean
  traceFlags: number
  traceState: string
  name: string
  timestamp: TelemetryTimestamp
  attributes: Record<string, unknown>
}

interface EndEvent {
  kind: 'end'
  traceId: string
  spanId: string
  timestamp: TelemetryTimestamp
  status: 'unset' | 'ok' | 'error'
  statusDescription?: string
  attributes: Record<string, unknown>
}

interface LogEvent {
  kind: 'log'
  traceId: string
  parentId: string
  severityNumber?: number
  severityText?: string
  timestamp: TelemetryTimestamp
  body?: unknown
  attributes: Record<string, unknown>
}

interface CloseEvent {
  kind: 'close'
  traceId: string
  spanId: string
  all: boolean
}

interface FlushEvent {
  kind: 'flush'
}

interface MetricEvent {
  kind: 'metric'
  instrumentKind: 'counter' | 'upDownCounter' | 'histogram'
  name: string
  unit: string
  description: string
  value: number
  attributes: Record<string, unknown>
}

type SpanEvent = StartEvent | EndEvent | LogEvent | CloseEvent | FlushEvent
type MetricHandle =
  | { kind: 'counter'; instrument: Counter }
  | { kind: 'upDownCounter'; instrument: UpDownCounter }
  | { kind: 'histogram'; instrument: Histogram }

interface SpanState {
  span?: Span
  context: Context
  root: string
  initialAttributes: Record<string, unknown>
}

let nativeInstalled = false
let owner: object | undefined
let components: TelemetryComponents | undefined
let acceptingTelemetry = false
let spansDisabled = false
let logsDisabled = false
let metricsDisabled = false
const spans = new Map<string, SpanState>()
const instruments = new Map<string, MetricHandle>()
const directOwner = {}

/**
 * Instrument Monty with standard OpenTelemetry components.
 *
 * Installation is process-wide and must happen before creating a pool. Each
 * signal is independently optional.
 */
export function instrumentTelemetry(value: TelemetryComponents): void {
  if (owner !== undefined) {
    throw new Error('Monty telemetry is already configured')
  }
  if (value.tracer === undefined && value.meter === undefined && value.logger === undefined) {
    throw new Error('at least one OpenTelemetry component is required')
  }
  activate(directOwner, value)
}

/** Wait until telemetry queued by the native pool has reached JavaScript. */
export async function flushTelemetry(): Promise<void> {
  await flushNativeTelemetry()
  if (!acceptingTelemetry) {
    components = undefined
    spans.clear()
    instruments.clear()
  }
}

/**
 * OpenTelemetry instrumentation for use with `NodeSDK` and compatible SDKs.
 *
 * Adding an instance to an SDK's `instrumentations` array explicitly enables
 * Monty's potentially sensitive telemetry. Configure it before creating pools.
 */
export class MontyInstrumentation {
  readonly instrumentationName = OTEL_SCOPE
  readonly instrumentationVersion = OTEL_VERSION

  private config: MontyInstrumentationConfig
  private tracerProvider: TracerProvider = TraceAPI.getTracerProvider()
  private meterProvider: MeterProvider = MetricsAPI.getMeterProvider()
  private active = false

  constructor(config: MontyInstrumentationConfig = {}) {
    this.config = { enabled: true, ...config }
    if (this.config.enabled) {
      this.enable()
    }
  }

  enable(): void {
    this.active = true
    this.refresh()
  }

  disable(): void {
    this.active = false
    deactivate(this)
  }

  setTracerProvider(provider: TracerProvider): void {
    this.tracerProvider = provider
    this.refresh()
  }

  setMeterProvider(provider: MeterProvider): void {
    this.meterProvider = provider
    this.refresh()
  }

  getConfig(): MontyInstrumentationConfig {
    return { ...this.config }
  }

  setConfig(config: MontyInstrumentationConfig): void {
    const wasEnabled = this.config.enabled !== false
    this.config = { enabled: true, ...config }
    const isEnabled = this.config.enabled !== false
    if (wasEnabled && !isEnabled) {
      this.disable()
    } else if (!wasEnabled && isEnabled) {
      this.enable()
    } else {
      this.refresh()
    }
  }

  /** Drain Monty's native callback queues before the SDK flushes providers. */
  async forceFlush(): Promise<void> {
    await flushTelemetry()
  }

  private refresh(): void {
    if (!this.active) {
      return
    }
    const value: TelemetryComponents = {}
    if (this.config.traces !== false) {
      value.tracer = this.tracerProvider.getTracer(this.instrumentationName, this.instrumentationVersion)
    }
    if (this.config.metrics !== false) {
      value.meter = this.meterProvider.getMeter(this.instrumentationName, this.instrumentationVersion)
    }
    if (this.config.logs !== false) {
      value.logger = globalLogger
    }
    activate(this, value)
  }
}

const globalLogger: Logger = {
  emit(record) {
    LogsAPI.getLogger(OTEL_SCOPE, OTEL_VERSION).emit(record)
  },
  enabled(options) {
    return LogsAPI.getLogger(OTEL_SCOPE, OTEL_VERSION).enabled(options)
  },
}

function activate(newOwner: object, value: TelemetryComponents): void {
  if (owner === directOwner && newOwner !== directOwner) {
    throw new Error('Monty telemetry is already configured')
  }
  installNativeCallbacks(value.meter !== undefined)
  setNativeMetricsEnabled(value.meter !== undefined)
  owner = newOwner
  components = value
  acceptingTelemetry = true
  spansDisabled = false
  logsDisabled = false
  metricsDisabled = false
  spans.clear()
  instruments.clear()
}

function deactivate(currentOwner: object): void {
  if (owner !== currentOwner) {
    return
  }
  owner = undefined
  acceptingTelemetry = false
  setNativeMetricsEnabled(false)
}

function installNativeCallbacks(metricsEnabled: boolean): void {
  if (nativeInstalled) {
    return
  }
  installNativeTelemetry(handleSpanEvent, handleMetricEvent, metricsEnabled)
  nativeInstalled = true
}

/** Captures distributed context synchronously before native `enter`. */
export function captureTelemetryContext(): Record<string, unknown> | undefined {
  const current = components
  if (!acceptingTelemetry || current === undefined) {
    return undefined
  }
  const tracingEnabled = current.tracer !== undefined && !spansDisabled
  const loggingEnabled = current.logger !== undefined && !logsDisabled
  if (!tracingEnabled && !loggingEnabled) {
    return undefined
  }
  if (!tracingEnabled) {
    return {}
  }
  try {
    const spanContext = TraceAPI.getSpanContext(ContextAPI.active())
    if (spanContext === undefined || !TraceAPI.isSpanContextValid(spanContext)) {
      return {}
    }
    return {
      traceId: spanContext.traceId,
      spanId: spanContext.spanId,
      traceFlags: spanContext.traceFlags,
      traceState: spanContext.traceState?.serialize(),
    }
  } catch {
    spansDisabled = true
    return current.logger === undefined || logsDisabled ? undefined : {}
  }
}

function handleSpanEvent(serialized: string): boolean {
  let event: SpanEvent
  try {
    event = JSON.parse(serialized) as SpanEvent
  } catch {
    return false
  }
  switch (event.kind) {
    case 'start':
      return startSpan(event)
    case 'end':
      endSpan(event)
      return true
    case 'log':
      emitLog(event)
      return true
    case 'close':
      closeSpans(event)
      return true
    case 'flush':
      return true
  }
}

function startSpan(event: StartEvent): boolean {
  const key = spanKey(event.traceId, event.spanId)
  const parentKey = event.parentId === null ? undefined : spanKey(event.traceId, event.parentId)
  const parent = parentKey === undefined ? undefined : spans.get(parentKey)
  const parentContext = parent?.context ?? externalParentContext(event)
  const root = parent?.root ?? key
  const tracer = components?.tracer
  if (tracer === undefined || spansDisabled) {
    return retainLogContext(event, key, root, parentContext)
  }
  try {
    const span = tracer.startSpan(
      event.name,
      {
        attributes: event.attributes as Attributes,
        startTime: timestamp(event.timestamp),
      },
      parentContext,
    )
    spans.set(key, {
      span,
      context: TraceAPI.setSpan(parentContext, span),
      root,
      initialAttributes: event.attributes,
    })
    return true
  } catch {
    spansDisabled = true
    return retainLogContext(event, key, root, parentContext)
  }
}

function endSpan(event: EndEvent): void {
  const key = spanKey(event.traceId, event.spanId)
  const state = spans.get(key)
  if (state === undefined) {
    return
  }
  spans.delete(key)
  if (state.span === undefined || spansDisabled) {
    return
  }
  try {
    const attributes = changedAttributes(event.attributes, state.initialAttributes)
    if (Object.keys(attributes).length !== 0) {
      state.span.setAttributes(attributes as Attributes)
    }
    if (event.status === 'ok') {
      state.span.setStatus({ code: SpanStatusCode.OK })
    } else if (event.status === 'error') {
      state.span.setStatus({ code: SpanStatusCode.ERROR, message: event.statusDescription })
    }
    state.span.end(timestamp(event.timestamp))
  } catch {
    spansDisabled = true
    if (components?.logger === undefined || logsDisabled) {
      spans.clear()
    }
  }
}

function emitLog(event: LogEvent): void {
  const logger = components?.logger
  if (logger === undefined || logsDisabled) {
    return
  }
  const parent = spans.get(spanKey(event.traceId, event.parentId))
  const record: LogRecord = {
    attributes: event.attributes as AnyValueMap,
    body: event.body as AnyValue,
    context: parent?.context ?? ROOT_CONTEXT,
    severityNumber: event.severityNumber as SeverityNumber | undefined,
    severityText: event.severityText,
    timestamp: timestamp(event.timestamp),
  }
  try {
    logger.emit(record)
  } catch {
    logsDisabled = true
  }
}

function closeSpans(event: CloseEvent): void {
  if (event.all) {
    spans.clear()
    return
  }
  const root = spanKey(event.traceId, event.spanId)
  for (const [key, state] of spans) {
    if (state.root === root) {
      spans.delete(key)
    }
  }
}

function externalParentContext(event: StartEvent): Context {
  if (event.parentId === null) {
    return ROOT_CONTEXT
  }
  return TraceAPI.setSpanContext(ROOT_CONTEXT, {
    traceId: event.traceId,
    spanId: event.parentId,
    traceFlags: event.traceFlags,
    traceState: event.traceState === '' ? undefined : createTraceState(event.traceState),
    isRemote: event.parentIsRemote,
  })
}

/** Retains native ancestry when logs are enabled without working span delivery. */
function retainLogContext(event: StartEvent, key: string, root: string, parentContext: Context): boolean {
  if (components?.logger === undefined || logsDisabled) {
    spans.clear()
    return false
  }
  const parentSpanContext = TraceAPI.getSpanContext(parentContext)
  spans.set(key, {
    context: TraceAPI.setSpanContext(parentContext, {
      traceId: parentSpanContext?.traceId ?? event.traceId,
      spanId: event.spanId,
      traceFlags: parentSpanContext?.traceFlags ?? event.traceFlags,
      traceState:
        parentSpanContext?.traceState ?? (event.traceState === '' ? undefined : createTraceState(event.traceState)),
      isRemote: false,
    }),
    root,
    initialAttributes: event.attributes,
  })
  return true
}

function handleMetricEvent(serialized: string): boolean {
  let event: MetricEvent | FlushEvent
  try {
    event = JSON.parse(serialized) as MetricEvent | FlushEvent
  } catch {
    return false
  }
  if (event.kind === 'flush') {
    return true
  }
  const meter = components?.meter
  if (meter === undefined || metricsDisabled) {
    return true
  }
  try {
    const handle = metricHandle(meter, event)
    if (handle.kind === 'histogram') {
      handle.instrument.record(event.value, event.attributes as Attributes, ROOT_CONTEXT)
    } else {
      handle.instrument.add(event.value, event.attributes as Attributes, ROOT_CONTEXT)
    }
  } catch {
    metricsDisabled = true
  }
  return true
}

function metricHandle(meter: Meter, event: MetricEvent): MetricHandle {
  const key = `${event.instrumentKind}:${event.name}`
  const existing = instruments.get(key)
  if (existing !== undefined) {
    return existing
  }
  const options = { description: event.description, unit: event.unit }
  let handle: MetricHandle
  if (event.instrumentKind === 'counter') {
    handle = { kind: event.instrumentKind, instrument: meter.createCounter(event.name, options) }
  } else if (event.instrumentKind === 'upDownCounter') {
    handle = { kind: event.instrumentKind, instrument: meter.createUpDownCounter(event.name, options) }
  } else {
    handle = { kind: event.instrumentKind, instrument: meter.createHistogram(event.name, options) }
  }
  instruments.set(key, handle)
  return handle
}

function spanKey(traceId: string, spanId: string): string {
  return `${traceId}:${spanId}`
}

/** Returns attributes added or changed after span creation. */
function changedAttributes(
  attributes: Record<string, unknown>,
  initialAttributes: Record<string, unknown>,
): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(attributes).filter(
      ([key, value]) => JSON.stringify(initialAttributes[key]) !== JSON.stringify(value),
    ),
  )
}

function timestamp(value: TelemetryTimestamp): [number, number] {
  return [Number(value.seconds), value.nanoseconds]
}
