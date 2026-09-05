import { context, ROOT_CONTEXT, trace } from '@opentelemetry/api'
import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks'
import type { Instrumentation } from '@opentelemetry/instrumentation'
import { InMemoryLogRecordExporter, LoggerProvider, SimpleLogRecordProcessor } from '@opentelemetry/sdk-logs'
import {
  AggregationTemporality,
  InMemoryMetricExporter,
  MeterProvider,
  PeriodicExportingMetricReader,
} from '@opentelemetry/sdk-metrics'
import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

import { CollectString, flushTelemetry, instrumentTelemetry, Monty, MontyInstrumentation } from '@pydantic/monty/node'

test('standard OpenTelemetry components receive Monty telemetry', async (ctx) => {
  skipIfBrowser(ctx)

  const instrumentation: Instrumentation = new MontyInstrumentation({ traces: false, metrics: false, logs: false })
  t.is(instrumentation.instrumentationName, '@pydantic/monty')
  t.is(instrumentation.getConfig().enabled, true)
  instrumentation.disable()

  const spanExporter = new InMemorySpanExporter()
  const tracerProvider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(spanExporter)] })
  const tracer = tracerProvider.getTracer('test')

  const logExporter = new InMemoryLogRecordExporter()
  const loggerProvider = new LoggerProvider({ processors: [new SimpleLogRecordProcessor({ exporter: logExporter })] })

  const metricExporter = new InMemoryMetricExporter(AggregationTemporality.CUMULATIVE)
  const metricReader = new PeriodicExportingMetricReader({ exporter: metricExporter, exportIntervalMillis: 60_000 })
  const meterProvider = new MeterProvider({
    readers: [metricReader],
    views: [{ instrumentName: 'monty.run.duration', name: 'monty.custom.run.duration' }],
  })

  t.throws(() => instrumentTelemetry({}), { message: 'at least one OpenTelemetry component is required' })
  instrumentTelemetry({
    tracer,
    meter: meterProvider.getMeter('test'),
    logger: loggerProvider.getLogger('test'),
  })
  t.throws(() => instrumentTelemetry({ tracer }), { message: 'Monty telemetry is already configured' })

  const contextManager = new AsyncLocalStorageContextManager().enable()
  context.setGlobalContextManager(contextManager)
  const parent = tracer.startSpan('parent', undefined, ROOT_CONTEXT)
  try {
    await context.with(trace.setSpan(ROOT_CONTEXT, parent), async () => {
      await using pool = await Monty.create()
      await using session = await pool.checkout({ scriptName: 'calculation.py' })
      const result = await session.feedRun("print('hello')\n'\\x00' * 70000", {
        printCallback: new CollectString(),
      })
      t.is((result as string).length, 70_000)
    })
  } finally {
    parent.end()
    context.disable()
    contextManager.disable()
  }

  await flushTelemetry()
  await meterProvider.forceFlush()

  const spans = spanExporter.getFinishedSpans()
  t.deepEqual(
    spans.map((span) => span.name),
    ['run code', 'session {script_name}', 'parent'],
  )
  const [run, session] = spans
  t.is(session?.parentSpanContext?.spanId, parent.spanContext().spanId)
  t.is(session?.parentSpanContext?.isRemote, false)
  t.is(run?.parentSpanContext?.spanId, session?.spanContext().spanId)
  t.is(session?.attributes.script_name, 'calculation.py')
  const output = run?.attributes.output
  t.is(typeof output, 'string')
  t.true(typeof output === 'string' && output.length < 70_000)
  t.is(run?.attributes.length_limit_exceeded, true)

  const [printed] = logExporter.getFinishedLogRecords()
  t.is(printed?.body, 'print stdout')
  t.is(printed?.attributes.text, 'hello\n')
  t.is(printed?.spanContext?.spanId, run?.spanContext().spanId)

  const metricNames = new Set(
    metricExporter
      .getMetrics()
      .flatMap((resource) => resource.scopeMetrics)
      .flatMap((scope) => scope.metrics)
      .map((metric) => metric.descriptor.name),
  )
  t.true(metricNames.has('monty.custom.run.duration'))
  t.false(metricNames.has('monty.run.duration'))

  await Promise.all([tracerProvider.shutdown(), loggerProvider.shutdown(), meterProvider.shutdown()])
})
