import { ROOT_CONTEXT, trace, type Context, type Meter, type Span, type Tracer } from '@opentelemetry/api'
import { type Logger } from '@opentelemetry/api-logs'
import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

import { _flushTelemetry, _installTelemetry, Monty } from '@pydantic/monty/node'

interface RecordedSpan {
  name: string
  parent?: Span
  recording: boolean
  attributes: Record<string, unknown>
  ended: boolean
}

interface RecordedMetric {
  name: string
  value: number
  attributes: Record<string, unknown>
}

test('standard OpenTelemetry components receive telemetry', async (ctx) => {
  skipIfBrowser(ctx)
  t.throws(() => _installTelemetry({}), {
    message: 'at least one OpenTelemetry component is required',
  })

  const spans: RecordedSpan[] = []
  let rejectNextRoot = true
  let nextSpanId = 0
  const tracer = {
    startSpan(name: string, options: { attributes?: Record<string, unknown> } = {}, parent: Context = ROOT_CONTEXT) {
      const parentSpan = trace.getSpan(parent)
      const recording = parentSpan?.isRecording() ?? !rejectNextRoot
      rejectNextRoot = false
      nextSpanId += 1
      const recorded: RecordedSpan = {
        name,
        parent: parentSpan,
        recording,
        attributes: { ...options.attributes },
        ended: false,
      }
      const span = {
        spanContext: () => ({
          traceId: '00000000000000000000000000000001',
          spanId: nextSpanId.toString(16).padStart(16, '0'),
          traceFlags: recording ? 1 : 0,
        }),
        setAttributes(attributes: Record<string, unknown>) {
          Object.assign(recorded.attributes, attributes)
          return this
        },
        setStatus() {
          return this
        },
        end() {
          recorded.ended = true
        },
        isRecording: () => recording,
      } as unknown as Span
      spans.push(recorded)
      return span
    },
  } as Tracer

  const logs: Array<{
    context?: Context
    severityNumber?: number
    severityText?: string
    body?: unknown
    attributes?: Record<string, unknown>
  }> = []
  const logger = {
    emit(record) {
      logs.push(record)
    },
  } as Logger

  const metrics: RecordedMetric[] = []
  const instrument = (name: string, method: 'add' | 'record') => ({
    [method](value: number, attributes: Record<string, unknown>) {
      metrics.push({ name, value, attributes })
    },
  })
  const meter = {
    createCounter: (name: string) => instrument(name, 'add'),
    createUpDownCounter: (name: string) => instrument(name, 'add'),
    createHistogram: (name: string) => instrument(name, 'record'),
  } as unknown as Meter

  _installTelemetry({ tracer, meter, logger })
  t.throws(() => _installTelemetry({ tracer }), {
    message: 'Monty telemetry is already configured',
  })

  await using pool = await Monty.create()
  await using rejected = await pool.checkout()
  t.is(await rejected.feedRun('1 + 2'), 3)
  await rejected.close()

  await using session = await pool.checkout()
  const result = await session.feedRun("print('hello')\n'\\x00' * 70000", { printCallback: () => {} })
  t.is((result as string).length, 70_000)
  await session.close()
  await _flushTelemetry()

  t.deepEqual(
    spans.map((span) => [span.name, span.recording, span.parent?.isRecording(), span.ended]),
    [
      ['session {script_name}', false, undefined, true],
      ['run code', false, false, true],
      ['session {script_name}', true, undefined, true],
      ['run code', true, true, true],
    ],
  )
  const output = spans[3]?.attributes.output
  t.is(typeof output, 'string')
  t.true((output as string).length < (result as string).length)
  t.is(spans[3]?.attributes.length_limit_exceeded, true)

  t.is(logs.length, 1)
  t.is(logs[0]?.body, 'print stdout')
  t.is(logs[0]?.severityNumber, 9)
  t.is(logs[0]?.severityText, 'INFO')
  t.is(logs[0]?.attributes?.text, 'hello\n')
  t.is(trace.getSpan(logs[0]?.context ?? ROOT_CONTEXT)?.isRecording(), true)

  t.true(metrics.some((metric) => metric.name === 'monty.run.duration' && metric.attributes.outcome === 'complete'))
  t.true(metrics.some((metric) => metric.name === 'monty.pool.workers.live'))
})
