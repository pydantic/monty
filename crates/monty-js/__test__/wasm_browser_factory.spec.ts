/// <reference types="vite/client" />
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'

import { createWorkerPool, loadModule } from '@pydantic/monty/wasm'
import workerUrl from '@pydantic/monty/wasm/worker?worker&url'

// A separate realm enforces Trusted Types without restricting the test runner.
describe.skipIf(typeof window === 'undefined')('browser worker factory', () => {
  let frame: HTMLIFrameElement
  let realm: typeof globalThis & {
    trustedTypes: {
      createPolicy(
        name: string,
        rules: { createScriptURL(url: string): string },
      ): {
        createScriptURL(url: string): unknown
      }
    }
  }

  beforeEach(() => {
    frame = document.createElement('iframe')
    document.body.append(frame)
    realm = frame.contentWindow as unknown as typeof realm
    const meta = frame.contentDocument!.createElement('meta')
    meta.httpEquiv = 'Content-Security-Policy'
    meta.content = "require-trusted-types-for 'script'; trusted-types montyTest"
    frame.contentDocument!.head.append(meta)
  })

  afterEach(() => frame.remove())

  test('rejects a plain worker URL under enforcement', () => {
    expect(() => new realm.Worker(workerUrl, { type: 'module' })).toThrow(/TrustedScriptURL/)
  })

  test('runs and recycles workers through the caller policy', async () => {
    const policy = realm.trustedTypes.createPolicy('montyTest', { createScriptURL: (url) => url })
    const workers: Worker[] = []
    const stopCounts: (() => number)[] = []
    const pool = await createWorkerPool(await loadModule(), {
      maxProcesses: 1,
      maxCheckoutsPerWorker: 1,
      workerUrl: '/must-not-be-used.js',
      createWorker: () => {
        const worker = new realm.Worker(policy.createScriptURL(workerUrl) as string, { type: 'module' })
        workers.push(worker)
        const stop = vi.spyOn(worker, 'terminate')
        stopCounts.push(() => stop.mock.calls.length)
        return worker
      },
    })
    try {
      for (const value of [42, 43]) {
        await using session = await pool.checkout()
        expect(await session.feedRun(String(value))).toBe(value)
      }
    } finally {
      await pool.close()
    }
    expect(workers).toHaveLength(2)
    expect(stopCounts.map((count) => count())).toEqual([1, 1])
    await expect(pool.checkout()).rejects.toThrow('closed')
  })

  test('propagates factory failures', async () => {
    const error = new Error('worker policy rejected the URL')
    await expect(
      createWorkerPool(
        {},
        {
          createWorker: () => {
            throw error
          },
        },
      ),
    ).rejects.toBe(error)
  })

  test('terminates a custom worker when component initialization fails', async () => {
    const policy = realm.trustedTypes.createPolicy('montyTest', { createScriptURL: (url) => url })
    const worker = new realm.Worker(policy.createScriptURL(workerUrl) as string, { type: 'module' })
    const stop = vi.spyOn(worker, 'terminate')
    await expect(createWorkerPool({}, { createWorker: () => worker })).rejects.toMatchObject({
      timedOut: false,
      message: 'worker initialization failed: component core module is missing: monty.component.core.wasm',
    })
    expect(stop.mock.calls).toEqual([[]])
  })
})
