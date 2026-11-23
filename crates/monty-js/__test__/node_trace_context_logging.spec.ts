// Per-turn trace context with `instrumentTelemetry({ logger })` alone: Monty records
// spans that no host tracer can resolve, so callbacks still join the server's span.

import { context } from '@opentelemetry/api'
import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks'
import { LoggerProvider } from '@opentelemetry/sdk-logs'
import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
import { afterAll, beforeAll, test } from 'vitest'
import { t } from './assertions.js'
import { serve, tracingRelay, type TestServer } from './remote_servers.js'

import { instrumentTelemetry, MontyRemote } from '@pydantic/monty/node'

const exporter = new InMemorySpanExporter()
const provider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] })
const tracer = provider.getTracer('trace-context')
const servers: TestServer[] = []

beforeAll(() => {
  context.setGlobalContextManager(new AsyncLocalStorageContextManager().enable())
  instrumentTelemetry({ logger: new LoggerProvider().getLogger('trace-context') })
})

afterAll(async () => {
  await Promise.all(servers.splice(0).map((s) => s.close()))
  context.disable()
  await provider.shutdown()
})

test('a callback runs under the span the server named without a Monty tracer', async () => {
  const sent: (string | undefined)[] = []
  const started = await serve(tracingRelay(sent))
  servers.push(started)
  await using pool = await MontyRemote.create(started.url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  const lookup = (key: string) => tracer.startActiveSpan('in lookup', (span) => (span.end(), key.toUpperCase()))

  const caller = await tracer.startActiveSpan('caller', async (span) => {
    t.is(await session.feedRun("lookup('a')", { externalLookup: { lookup } }), 'A')
    span.end()
    return span.spanContext()
  })

  t.deepEqual(sent, [`00-${caller.traceId}-${caller.spanId}-0${caller.traceFlags}`])
  const [inLookup] = exporter.getFinishedSpans().filter((span) => span.name === 'in lookup')
  t.deepEqual(
    [inLookup?.spanContext().traceId, inLookup?.parentSpanContext?.spanId],
    [caller.traceId, '0000000000000001'],
  )
})
