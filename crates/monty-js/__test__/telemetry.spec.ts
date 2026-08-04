import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

import { _installTelemetryAdapter, Monty, type TelemetryEvent } from '@pydantic/monty/node'

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
  t.is(await session.feedRun('1 + 2'), 3)
  await session.close()
  await new Promise((resolve) => setTimeout(resolve, 20))

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
  t.deepEqual(
    events.map((event) => event.kind),
    ['start', 'start', 'end', 'end'],
  )
})
