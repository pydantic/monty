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
    import { context, trace, propagation, createContextKey } from '@opentelemetry/api'
    import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks'
    import { AlwaysOffSampler, BasicTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } from '@opentelemetry/sdk-trace-base'
    import { Monty, MontyComplete, FunctionSnapshot, NameLookupSnapshot, FutureSnapshot, instrumentTelemetry, flushTelemetry } from ${JSON.stringify(new URL('../dist/node.js', import.meta.url).href)}

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

test.each([
  ['function', 'callback()', 'call {function_name}'],
  ['os', "from pathlib import Path\nPath('/file').exists()", 'os call {function}'],
  ['name', 'missing', 'name lookup {name}'],
  ['future', 'await callback()', 'resolve futures'],
])('manual snapshot context: %s', (kind, code, parentName) => {
  runChild(`
    instrumentTelemetry({ tracer })
    const pool = await Monty.create()
    const session = await pool.checkout()
    await storage.run('caller', () => tracer.startActiveSpan('host', async host => {
      try {
        const baggage = propagation.createBaggage({ request: { value: 'manual' } })
        const key = createContextKey('custom entry')
        const feedContext = propagation.setBaggage(context.active(), baggage).setValue(key, 'feed value')
        let paused = await context.with(feedContext, () => session.feedStart(${JSON.stringify(code)}))
        if (${JSON.stringify(kind)} === 'future') paused = await paused.resumeFuture()
        const callerBaggage = propagation.createBaggage({ request: { value: 'caller' } })
        await context.with(propagation.setBaggage(context.active(), callerBaggage), async () => {
          const saved = paused.traceContext()
          assert.equal(saved.getValue(key), 'feed value')
          assert.equal(propagation.getBaggage(context.active()).getEntry('request').value, 'caller')
          const suspension = trace.getSpan(saved)
          assert.notEqual(suspension, host)
          assert.equal(trace.getSpan(context.active()), host)
          const result = await context.with(saved, async () => {
            assert.equal(trace.getSpan(context.active()), suspension)
            assert.equal(propagation.getBaggage(context.active()).getEntry('request').value, 'manual')
            return tracer.startActiveSpan('handler', async child => {
              try {
                await new Promise(resolve => setTimeout(resolve, 1))
                assert.equal(trace.getSpan(context.active()), child)
                assert.equal(storage.getStore(), 'caller')
                return 42
              } finally { child.end() }
            })
          })
          assert.equal(trace.getSpan(context.active()), host)
          assert.equal(suspension.isRecording(), true)
          assert.throws(() => context.with(saved, () => { throw new Error('handler failed') }), {
            message: 'handler failed',
          })
          await assert.rejects(context.with(saved, async () => {
            await Promise.resolve()
            throw new Error('async handler failed')
          }), { message: 'async handler failed' })
          assert.equal(trace.getSpan(context.active()), host)
          const done = paused instanceof NameLookupSnapshot
            ? await paused.resumeValue(result)
            : paused instanceof FutureSnapshot
              ? await paused.resume([{ callId: paused.pendingCallIds[0], value: result }])
              : await paused.resume(result)
          assert.equal(done.output, 42)
          assert.throws(() => paused.traceContext(), { message: 'snapshot has already been resumed' })
          await flushTelemetry()
          assert.equal(suspension.isRecording(), false)
          context.with(saved, () => assert.equal(trace.getSpan(context.active()), suspension))
          assert.equal(trace.getSpan(context.active()), host)
        })
      } finally { host.end() }
    }))
    await session.close()
    await pool.close()
    await flushTelemetry()
    const spans = exporter.getFinishedSpans()
    const handler = spans.find(span => span.name === 'handler')
    const parent = spans.find(span => span.spanContext().spanId === handler.parentSpanContext.spanId)
    assert.equal(parent.name, ${JSON.stringify(parentName)})
  `)
})

test('manual snapshot contexts stay isolated across concurrent async handlers', () => {
  runChild(`
    instrumentTelemetry({ tracer })
    const pool = await Monty.create({ minProcesses: 2, maxProcesses: 2 })
    let entered = 0
    let release
    const ready = new Promise(resolve => { release = resolve })
    const parents = await Promise.all([1, 2].map(index => storage.run(index, () =>
      tracer.startActiveSpan('host ' + index, async host => {
        const session = await pool.checkout()
        try {
          const baggage = propagation.createBaggage({ request: { value: String(index) } })
          const paused = await context.with(propagation.setBaggage(context.active(), baggage), () => session.feedStart('callback()'))
          const suspension = trace.getSpan(paused.traceContext())
          const value = await context.with(paused.traceContext(), async () => {
            if (++entered === 2) release()
            await ready
            assert.equal(propagation.getBaggage(context.active()).getEntry('request').value, String(index))
            assert.equal(trace.getSpan(context.active()), suspension)
            assert.equal(storage.getStore(), index)
            tracer.startActiveSpan('handler ' + index, span => span.end())
            return index
          })
          assert.equal(trace.getSpan(context.active()), host)
          assert.equal((await paused.resume(value)).output, index)
          return suspension.spanContext().spanId
        } finally { await session.close(); host.end() }
      })
    )))
    await pool.close()
    await flushTelemetry()
    assert.notEqual(parents[0], parents[1])
    const spans = exporter.getFinishedSpans()
    for (const index of [1, 2]) {
      const handler = spans.find(span => span.name === 'handler ' + index)
      assert.equal(handler.parentSpanContext.spanId, parents[index - 1])
    }
  `)
})

test.each(['disabled', 'broken-tracer', 'broken-context', 'sampled-out'])('snapshot context fallback: %s', (mode) => {
  runChild(`
    const mode = ${JSON.stringify(mode)}
    const offProvider = new BasicTracerProvider({ sampler: new AlwaysOffSampler() })
    const offTracer = offProvider.getTracer('not-recording')
    let callSpan
    if (mode !== 'disabled') instrumentTelemetry({ tracer: {
      startSpan(name, ...args) {
        if (mode === 'broken-tracer') throw new Error('tracer failed')
        const span = (mode === 'sampled-out' ? offTracer : tracer).startSpan(name, ...args)
        if (name === 'call {function_name}') callSpan = span
        return span
      },
    } })
    await tracer.startActiveSpan('host', async host => {
      const pool = await Monty.create()
      const session = await pool.checkout()
      const originalSetSpan = trace.setSpan
      try {
        const paused = await session.feedStart('callback()')
        if (mode === 'broken-context') trace.setSpan = () => { throw new Error('context failed') }
        const key = createContextKey('after feed')
        const saved = context.with(context.active().setValue(key, 'caller'), () => paused.traceContext())
        assert.equal(saved.getValue(key), undefined)
        assert.equal(trace.getSpan(saved), mode === 'sampled-out' ? callSpan : host)
        assert.equal(trace.getSpan(context.active()), host)
        await context.with(saved, async () => {
          await Promise.resolve()
          assert.equal(trace.getSpan(context.active()), mode === 'sampled-out' ? callSpan : host)
        })
        assert.equal(trace.getSpan(context.active()), host)
        if (mode === 'sampled-out') assert.equal(callSpan.isRecording(), false)
        trace.setSpan = originalSetSpan
        assert.equal((await paused.resume(42)).output, 42)
      } finally {
        trace.setSpan = originalSetSpan
        await session.close()
        await pool.close()
        host.end()
      }
    })
    await offProvider.shutdown()
  `)
})

test('restored snapshots expose their new suspension context', () => {
  runChild(`
    instrumentTelemetry({ tracer })
    const pool = await Monty.create()
    const session = await pool.checkout()
    const originalBaggage = propagation.createBaggage({ request: { value: 'original' } })
    const original = await context.with(propagation.setBaggage(context.active(), originalBaggage), () => session.feedStart('callback()'))
    const originalSpan = trace.getSpan(original.traceContext())
    const dump = await original.dump()
    await session.close()
    const restoredSession = await pool.checkout()
    const restoredBaggage = propagation.createBaggage({ request: { value: 'restored' } })
    const restored = await context.with(propagation.setBaggage(context.active(), restoredBaggage), () => restoredSession.loadSnapshot(dump))
    const restoredContext = restored.traceContext()
    assert.equal(propagation.getBaggage(restoredContext).getEntry('request').value, 'restored')
    const restoredSpan = trace.getSpan(restoredContext)
    assert.notEqual(restoredSpan.spanContext().spanId, originalSpan.spanContext().spanId)
    context.with(restoredContext, () => tracer.startActiveSpan('restored handler', span => span.end()))
    assert.equal((await restored.resume(42)).output, 42)
    await restoredSession.close()
    await pool.close()
    await flushTelemetry()
    const handler = exporter.getFinishedSpans().find(span => span.name === 'restored handler')
    assert.equal(handler.parentSpanContext.spanId, restoredSpan.spanContext().spanId)
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
