import { execFileSync } from 'node:child_process'

import { test } from 'vitest'

test('MontyInstrumentation accepts SDK providers', () => {
  runTelemetryChild(`
    import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
    import { Monty, MontyInstrumentation } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const exporter = new InMemorySpanExporter()
    const provider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] })
    const instrumentation = new MontyInstrumentation({ logs: false, metrics: false })
    instrumentation.setTracerProvider(provider)
    const pool = await Monty.create()
    const session = await pool.checkout()
    if (await session.feedRun('1 + 2') !== 3) throw new Error('unexpected result')
    await session.close()
    await pool.close()
    await instrumentation.forceFlush()
    const names = exporter.getFinishedSpans().map((span) => span.name)
    if (JSON.stringify(names) !== JSON.stringify(['run code', 'session {script_name}'])) {
      throw new Error(JSON.stringify(names))
    }
    instrumentation.disable()
    await provider.shutdown()
  `)
})

test('a second MontyInstrumentation is rejected', () => {
  runTelemetryChild(`
    import { MontyInstrumentation } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const first = new MontyInstrumentation({ logs: false, metrics: false, traces: false })
    let message
    try {
      new MontyInstrumentation({ logs: false, metrics: false, traces: false })
    } catch (error) {
      message = error instanceof Error ? error.message : String(error)
    }
    if (message !== 'Monty telemetry is already configured') throw new Error(String(message))
    first.disable()
  `)
})

test('disable stops telemetry from an active session', () => {
  runTelemetryChild(`
    import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
    import { Monty, MontyInstrumentation } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const exporter = new InMemorySpanExporter()
    const provider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] })
    const instrumentation = new MontyInstrumentation({ logs: false, metrics: false })
    instrumentation.setTracerProvider(provider)
    const pool = await Monty.create()
    const session = await pool.checkout()
    if (await session.feedRun('1 + 2') !== 3) throw new Error('unexpected result')
    await instrumentation.forceFlush()
    instrumentation.disable()
    if (await session.feedRun('4 + 5') !== 9) throw new Error('unexpected result')
    await session.close()
    await pool.close()
    await instrumentation.forceFlush()
    const names = exporter.getFinishedSpans().map((span) => span.name)
    if (JSON.stringify(names) !== JSON.stringify(['run code'])) throw new Error(JSON.stringify(names))
    await provider.shutdown()
  `)
})

test('concurrent checkouts deliver complete span trees', () => {
  runTelemetryChild(`
    import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
    import { flushTelemetry, instrumentTelemetry, Monty } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const exporter = new InMemorySpanExporter()
    const provider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] })
    instrumentTelemetry({ tracer: provider.getTracer('test') })
    const pool = await Monty.create({ minProcesses: 2, maxProcesses: 2 })
    await Promise.all([1, 2].map(async (value) => {
      const session = await pool.checkout()
      if (await session.feedRun('value + 1', { inputs: { value } }) !== value + 1) throw new Error('unexpected result')
      await session.close()
    }))
    await pool.close()
    await flushTelemetry()
    const spans = exporter.getFinishedSpans()
    if (spans.length !== 4 || new Set(spans.map((span) => span.spanContext().traceId)).size !== 2) {
      throw new Error(JSON.stringify(spans.map((span) => span.name)))
    }
    await provider.shutdown()
  `)
})

test('host sampling can reject one child without disabling its root', () => {
  runTelemetryChild(`
    import { BasicTracerProvider, InMemorySpanExporter, SamplingDecision, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
    import { flushTelemetry, instrumentTelemetry, Monty } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    let rejected = false
    const sampler = {
      shouldSample(_context, _traceId, name) {
        if (name === 'run code' && !rejected) {
          rejected = true
          return { decision: SamplingDecision.NOT_RECORD }
        }
        return { decision: SamplingDecision.RECORD_AND_SAMPLED }
      },
      toString() { return 'RejectFirstRun' },
    }
    const exporter = new InMemorySpanExporter()
    const provider = new BasicTracerProvider({
      sampler,
      spanProcessors: [new SimpleSpanProcessor(exporter)],
    })
    instrumentTelemetry({ tracer: provider.getTracer('test') })
    const pool = await Monty.create()
    const session = await pool.checkout()
    if (await session.feedRun('1 + 2') !== 3) throw new Error('unexpected result')
    if (await session.feedRun('4 + 5') !== 9) throw new Error('unexpected result')
    await session.close()
    await pool.close()
    await flushTelemetry()
    const names = exporter.getFinishedSpans().map((span) => span.name)
    if (JSON.stringify(names) !== JSON.stringify(['run code', 'session {script_name}'])) {
      throw new Error(JSON.stringify(names))
    }
    await provider.shutdown()
  `)
})

test('logging failure does not disable tracing', () => {
  runTelemetryChild(`
    import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
    import { flushTelemetry, instrumentTelemetry, Monty } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const exporter = new InMemorySpanExporter()
    const provider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] })
    instrumentTelemetry({
      tracer: provider.getTracer('test'),
      logger: { emit() { throw new Error('logging failed') }, enabled() { return true } },
    })
    const pool = await Monty.create()
    const session = await pool.checkout()
    if (await session.feedRun("print('hello')\\n1 + 2") !== 3) throw new Error('unexpected result')
    await session.close()
    await pool.close()
    await flushTelemetry()
    const names = exporter.getFinishedSpans().map((span) => span.name)
    if (JSON.stringify(names) !== JSON.stringify(['run code', 'session {script_name}'])) {
      throw new Error(JSON.stringify(names))
    }
    await provider.shutdown()
  `)
})

test('metric failure does not disable tracing', () => {
  runTelemetryChild(`
    import { BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
    import { flushTelemetry, instrumentTelemetry, Monty } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const exporter = new InMemorySpanExporter()
    const provider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] })
    const broken = { add() { throw new Error('metrics failed') }, record() { throw new Error('metrics failed') } }
    instrumentTelemetry({
      tracer: provider.getTracer('test'),
      meter: {
        createCounter() { return broken },
        createUpDownCounter() { return broken },
        createHistogram() { return broken },
      },
    })
    const pool = await Monty.create()
    const session = await pool.checkout()
    if (await session.feedRun('1 + 2') !== 3) throw new Error('unexpected result')
    await session.close()
    await pool.close()
    await flushTelemetry()
    const names = exporter.getFinishedSpans().map((span) => span.name)
    if (JSON.stringify(names) !== JSON.stringify(['run code', 'session {script_name}'])) {
      throw new Error(JSON.stringify(names))
    }
    await provider.shutdown()
  `)
})

test('logger-only installation preserves native trace context', () => {
  runTelemetryChild(`
    import { InMemoryLogRecordExporter, LoggerProvider, SimpleLogRecordProcessor } from '@opentelemetry/sdk-logs'
    import { flushTelemetry, instrumentTelemetry, Monty } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const exporter = new InMemoryLogRecordExporter()
    const provider = new LoggerProvider({ processors: [new SimpleLogRecordProcessor({ exporter })] })
    instrumentTelemetry({ logger: provider.getLogger('test') })
    const pool = await Monty.create()
    const session = await pool.checkout()
    if (await session.feedRun("print('hello')\\n1 + 2") !== 3) throw new Error('unexpected result')
    await session.close()
    await pool.close()
    await flushTelemetry()
    const [record] = exporter.getFinishedLogRecords()
    if (record?.spanContext?.traceId === undefined || record.spanContext.spanId === undefined) {
      throw new Error(JSON.stringify(record))
    }
    await provider.shutdown()
  `)
})

test('tracing failure does not disable logging', () => {
  runTelemetryChild(`
    import { InMemoryLogRecordExporter, LoggerProvider, SimpleLogRecordProcessor } from '@opentelemetry/sdk-logs'
    import { flushTelemetry, instrumentTelemetry, Monty } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const exporter = new InMemoryLogRecordExporter()
    const provider = new LoggerProvider({ processors: [new SimpleLogRecordProcessor({ exporter })] })
    instrumentTelemetry({
      tracer: { startSpan() { throw new Error('tracing failed') } },
      logger: provider.getLogger('test'),
    })
    const pool = await Monty.create()
    const session = await pool.checkout()
    if (await session.feedRun("print('still logged')\\n1 + 2") !== 3) throw new Error('unexpected result')
    await session.close()
    await pool.close()
    await flushTelemetry()
    const [record] = exporter.getFinishedLogRecords()
    if (record?.body !== 'print stdout' || record.attributes.text !== 'still logged\\n') {
      throw new Error(JSON.stringify(record))
    }
    await provider.shutdown()
  `)
})

function runTelemetryChild(source: string): void {
  try {
    execFileSync(process.execPath, ['--input-type=module', '--eval', source], {
      cwd: new URL('..', import.meta.url),
      stdio: 'pipe',
      timeout: 30_000,
    })
  } catch (error) {
    const failure = error as Error & { stderr?: Buffer; stdout?: Buffer }
    const output = [failure.stdout?.toString(), failure.stderr?.toString()].filter(Boolean).join('\n')
    throw new Error(output === '' ? failure.message : `${failure.message}\n${output}`, { cause: error })
  }
}
