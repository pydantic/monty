import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

import { _installTelemetryAdapter, Monty, type TelemetryEvent, type TelemetrySpanEvent } from '@pydantic/monty/node'

test('installed telemetry adapter receives the session tree', async (ctx) => {
  skipIfBrowser(ctx)
  const events: TelemetryEvent[] = []
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
  const spanEvents = () => events.filter((event) => event.kind !== 'metric')
  const deadline = Date.now() + 2_000
  while (spanEvents().length < 4 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 10))
  }

  const starts = events.filter((event): event is TelemetrySpanEvent => event.kind === 'start')
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
    spanEvents().map((event) => event.kind),
    ['start', 'start', 'end', 'end'],
  )

  // metrics travel over the same callback but carry no trace context
  const metrics = events.filter((event) => event.kind === 'metric')
  const run = metrics.find((event) => event.name === 'monty.run.duration')
  t.is(run?.metricKind, 'histogram')
  t.is(run?.unit, 's')
  t.deepEqual(run?.attributes, { outcome: 'complete' })
  t.true((run?.value ?? 0) > 0)
  const workers = metrics.filter((event) => event.name === 'monty.pool.workers.live')
  t.true(workers.length > 0)
  t.is(workers[0]?.metricKind, 'up_down_counter')
})
