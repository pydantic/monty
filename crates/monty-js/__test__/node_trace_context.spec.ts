// Per-turn trace context against a server that records spans (see `tracingRelay`
// in remote_servers.ts), without `instrumentTelemetry()`: only
// `@opentelemetry/api` reads and sets the active span.

import { context, type SpanContext } from '@opentelemetry/api'
import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks'
import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
import { afterAll, afterEach, beforeAll, test } from 'vitest'
import { t } from './assertions.js'
import { serve, tracingRelay, type TestServer } from './remote_servers.js'

import { FunctionSnapshot, MontyComplete, MontyRemote } from '@pydantic/monty'

const exporter = new InMemorySpanExporter()
const provider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] })
const tracer = provider.getTracer('trace-context')
const servers: TestServer[] = []

beforeAll(() => {
  context.setGlobalContextManager(new AsyncLocalStorageContextManager().enable())
})

afterAll(async () => {
  context.disable()
  await provider.shutdown()
})

afterEach(async () => {
  exporter.reset()
  await Promise.all(servers.splice(0).map((s) => s.close()))
})

function traceparent(span: SpanContext): string {
  return `00-${span.traceId}-${span.spanId}-0${span.traceFlags}`
}

/** The trace and parent span of every finished span called `name`, oldest first. */
function parents(name: string): [string, string | undefined][] {
  return exporter
    .getFinishedSpans()
    .filter((span) => span.name === name)
    .map((span) => [span.spanContext().traceId, span.parentSpanContext?.spanId])
}

test('each feed names its caller span, and a callback runs under the span the server named', async () => {
  const sent: (string | undefined)[] = []
  const started = await serve(tracingRelay(sent))
  servers.push(started)
  await using pool = await MontyRemote.create(started.url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  const lookup = (key: string) => tracer.startActiveSpan('in lookup', (span) => (span.end(), key.toUpperCase()))

  const callers: SpanContext[] = []
  for (const name of ['first caller', 'second caller']) {
    await tracer.startActiveSpan(name, async (span) => {
      callers.push(span.spanContext())
      t.is(await session.feedRun("lookup('a')", { externalLookup: { lookup } }), 'A')
      span.end()
    })
  }
  // no active span: the request names none
  t.is(await session.feedRun('1 + 1'), 2)

  t.deepEqual(sent, [traceparent(callers[0]!), traceparent(callers[1]!), undefined])
  t.deepEqual(parents('in lookup'), [
    [callers[0]!.traceId, '0000000000000001'],
    [callers[1]!.traceId, '0000000000000002'],
  ])
})

test('a snapshot carries the span the server named for its suspension', async () => {
  const sent: (string | undefined)[] = []
  const started = await serve(tracingRelay(sent))
  servers.push(started)
  await using pool = await MontyRemote.create(started.url, { requestTimeout: 30 })
  await using session = await pool.checkout()

  const caller = await tracer.startActiveSpan('caller', async (span) => {
    const snapshot = await session.feedStart("lookup('a')")
    span.end()
    t.true(snapshot instanceof FunctionSnapshot)
    context.with((snapshot as FunctionSnapshot).traceContext(), () => tracer.startSpan('handler').end())
    t.true((await (snapshot as FunctionSnapshot).resume('A')) instanceof MontyComplete)
    return span.spanContext()
  })

  t.deepEqual(sent, [traceparent(caller)])
  t.deepEqual(parents('handler'), [[caller.traceId, '0000000000000001']])
})
