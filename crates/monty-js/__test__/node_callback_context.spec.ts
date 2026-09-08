import { execFileSync } from 'node:child_process'

import { test } from 'vitest'

function runChild(source: string): void {
  execFileSync(
    process.execPath,
    [
      '--input-type=module',
      '-e',
      `
    import assert from 'node:assert/strict'
    import { AsyncLocalStorage } from 'node:async_hooks'
    import { context, trace, propagation } from '@opentelemetry/api'
    import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks'
    import { AlwaysOffSampler, BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
    import { Monty, MontyComplete, instrumentTelemetry, flushTelemetry } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

    const manager = new AsyncLocalStorageContextManager().enable()
    context.setGlobalContextManager(manager)
    const storage = new AsyncLocalStorage()
    const exporter = new InMemorySpanExporter()
    const provider = new BasicTracerProvider({ spanProcessors: [new SimpleSpanProcessor(exporter)] })
    const tracer = provider.getTracer('callbacks')
    ${source}
    await flushTelemetry()
    await provider.shutdown()
    context.disable()
    storage.disable()
  `,
    ],
    { stdio: 'pipe', timeout: 30_000 },
  )
}

test('concurrent host callbacks inherit their Monty span and caller async storage', () => {
  runChild(`
    instrumentTelemetry({ tracer })
    const pool = await Monty.create({ minProcesses: 2, maxProcesses: 2 })
    await Promise.all([1, 2].map(index => storage.run(index, () => {
      const baggage = propagation.createBaggage({ request: { value: String(index) } })
      return context.with(propagation.setBaggage(context.active(), baggage), () => tracer.startActiveSpan('host ' + index, async host => {
        const check = () => {
          assert.equal(storage.getStore(), index)
          assert.equal(propagation.getBaggage(context.active()).getEntry('request').value, String(index))
        }
        const child = name => {
          check()
          tracer.startActiveSpan(name + ' ' + index, span => span.end())
        }
        try {
          const session = await pool.checkout({ scriptName: String(index) })
          assert.equal(await session.feedRun("print('hello'); sync_callback() + await async_callback()", {
            printCallback() { child('print') },
            externalLookup: {
              sync_callback() { child('sync'); return 10 },
              async async_callback() {
                check()
                return tracer.startActiveSpan('async ' + index, async span => {
                  try {
                    await new Promise(resolve => setTimeout(resolve, 10))
                    check()
                    assert.equal(trace.getSpan(context.active()), span)
                    return 20
                  } finally { span.end() }
                })
              },
            },
          }), 30)
          assert.equal(await session.feedRun("from pathlib import Path; Path('/x').exists()", {
            os() { child('os'); return true },
          }), true)
          assert.equal(await session.feedRun('lazy_value', {
            externalLookup: { get lazy_value() { child('lookup'); return 7 } },
          }), 7)
          assert.equal(trace.getSpan(context.active()), host)
          check()
          await session.close()
        } finally { host.end() }
      }))
    })))
    await pool.close()
    await flushTelemetry()
    assert.equal(storage.getStore(), undefined)
    assert.equal(trace.getSpan(context.active()), undefined)
    const spans = exporter.getFinishedSpans()
    const byId = new Map(spans.map(span => [span.spanContext().spanId, span]))
    for (const index of [1, 2]) {
      for (const [kind, expected] of [['print', 'run code'], ['sync', 'call {function_name}'], ['async', 'call {function_name}'], ['os', 'os call {function}'], ['lookup', 'name lookup {name}']]) {
        const span = spans.find(span => span.name === kind + ' ' + index)
        assert.ok(span, kind)
        let parent = byId.get(span.parentSpanContext.spanId)
        assert.equal(parent.name, expected)
        while (parent.parentSpanContext) parent = byId.get(parent.parentSpanContext.spanId)
        assert.equal(parent.name, 'host ' + index)
      }
    }
  `)
})

test.each(['disabled', 'broken-tracer', 'broken-context', 'sampled-out'])('callback context fallback: %s', (mode) => {
  runChild(`
    const mode = ${JSON.stringify(mode)}
    const offProvider = new BasicTracerProvider({ sampler: new AlwaysOffSampler() })
    const offTracer = offProvider.getTracer('not-recording')
    let runSpan
    if (mode !== 'disabled') instrumentTelemetry({ tracer: {
      startSpan(name, ...args) {
        if (mode === 'broken-tracer') throw new Error('tracer failed')
        const span = (mode === 'sampled-out' ? offTracer : tracer).startSpan(name, ...args)
        if (name === 'run code') runSpan = span
        return span
      },
    } })
    await storage.run('caller', () => tracer.startActiveSpan('host', async host => {
      const originalWith = context.with
      if (mode === 'broken-context') context.with = () => { throw new Error('context failed') }
      let count = 0
      try {
        const pool = await Monty.create()
        const session = await pool.checkout()
        assert.equal(await session.feedRun("print('hello'); 42", { printCallback() {
          count++
          assert.equal(storage.getStore(), 'caller')
          assert.equal(trace.getSpan(context.active()), mode === 'sampled-out' ? runSpan : host)
          if (mode === 'sampled-out') assert.equal(runSpan.isRecording(), false)
        } }), 42)
        assert.equal(count, 1)
        assert.equal(trace.getSpan(context.active()), host)
        await session.close()
        await pool.close()
      } finally { context.with = originalWith; host.end() }
    }))
    await offProvider.shutdown()
  `)
})

test('callback failures are not retried and do not leak context', () => {
  runChild(`
    instrumentTelemetry({ tracer })
    await storage.run('caller', () => tracer.startActiveSpan('host', async host => {
      const pool = await Monty.create()
      let count = 0
      try {
        const session = await pool.checkout()
        await assert.rejects(session.feedRun("print('hello')", { printCallback() {
          count++
          assert.equal(storage.getStore(), 'caller')
          throw new Error('print failed')
        } }), /print failed/)
        assert.equal(trace.getSpan(context.active()), host)
        await session.close()
        const second = await pool.checkout()
        await assert.rejects(second.feedRun('fail()', { externalLookup: { fail() {
          count++
          throw new Error('function failed')
        } } }), /function failed/)
        assert.equal(trace.getSpan(context.active()), host)
        await assert.rejects(second.feedRun('await fail()', { externalLookup: { async fail() {
          count++
          await Promise.resolve()
          throw new Error('async failed')
        } } }), /async failed/)
        assert.equal(trace.getSpan(context.active()), host)
        assert.equal(count, 3)
        await second.close()
      } finally { await pool.close(); host.end() }
    }))
  `)
})

test('snapshot resumes capture the resuming caller storage and retain the Monty parent', () => {
  runChild(`
    instrumentTelemetry({ tracer })
    const pool = await Monty.create()
    const session = await pool.checkout()
    let snapshot = await storage.run('feed', () => session.feedStart("value = await callback(); print(value); value", {
      externalLookup: { async callback() {
        assert.equal(storage.getStore(), 'resume')
        return tracer.startActiveSpan('snapshot callback', async span => {
          try {
            await Promise.resolve()
            assert.equal(storage.getStore(), 'resume')
            return 42
          } finally { span.end() }
        })
      } },
      printCallback() {
        assert.equal(storage.getStore(), 'resume')
        tracer.startActiveSpan('snapshot print', span => span.end())
      },
    }))
    await storage.run('resume', async () => {
      while (!(snapshot instanceof MontyComplete)) snapshot = await snapshot.resumeAuto()
    })
    assert.equal(snapshot.output, 42)
    await session.close()
    await pool.close()
    await flushTelemetry()
    const spans = exporter.getFinishedSpans()
    for (const [name, expected] of [['snapshot callback', 'call {function_name}'], ['snapshot print', 'run code']]) {
      const span = spans.find(span => span.name === name)
      const parent = spans.find(parent => parent.spanContext().spanId === span.parentSpanContext.spanId)
      assert.equal(parent.name, expected)
    }
  `)
})
