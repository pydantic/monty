import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

import { _flushTelemetry, _installTelemetryAdapter, Monty, type TelemetryEvent } from '@pydantic/monty/node'

test('installed telemetry adapter receives the session tree', async (ctx) => {
  skipIfBrowser(ctx)
  const events: TelemetryEvent[] = []
  const metricBatches: Uint8Array[] = []
  const adapter = {
    captureContext() {
      return {
        traceId: '00000000000000000000000000000001',
        spanId: '0000000000000002',
        traceFlags: 1,
      }
    },
    event(event: TelemetryEvent) {
      events.push(event)
    },
    exportMetrics(payload: Uint8Array) {
      metricBatches.push(payload)
      return Promise.reject(new Error('telemetry export failed'))
    },
  }
  t.throws(() => _installTelemetryAdapter(2, adapter), {
    message: 'unsupported Monty telemetry adapter version 2; expected 1',
  })
  _installTelemetryAdapter(1, adapter)

  await using pool = await Monty.create()
  await using session = await pool.checkout()
  const result = await session.feedRun("'\\x00' * 70000")
  t.is((result as string).length, 70_000)
  await session.close()
  const deadline = Date.now() + 2_000
  while (events.length < 4 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 10))
  }

  const starts = events.filter((event) => event.kind === 'start')
  t.deepEqual(
    starts.map((event) => [event.traceId, event.traceFlags, event.traceState, event.name]),
    [
      ['00000000000000000000000000000001', 1, '', 'session {script_name}'],
      ['00000000000000000000000000000001', 1, '', 'run code'],
    ],
  )
  t.is(starts[0]?.parentId, '0000000000000002')
  t.is(starts[1]?.parentId, starts[0]?.spanId)
  const runEnd = events.find((event) => event.kind === 'end' && event.spanId === starts[1]?.spanId)
  const output = runEnd?.attributes?.output
  t.is(typeof output, 'string')
  t.true((output as string).length < (result as string).length)
  t.is(runEnd?.attributes?.length_limit_exceeded, true)
  t.deepEqual(
    events.map((event) => event.kind),
    ['start', 'start', 'end', 'end'],
  )

  // metrics are aggregated in Rust and cross as a canonical OTLP protobuf
  await _flushTelemetry()
  t.is(metricBatches.length, 1)
  t.true((metricBatches[0]?.byteLength ?? 0) > 0)
})
