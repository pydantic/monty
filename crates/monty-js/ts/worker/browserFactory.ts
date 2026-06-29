/// <reference lib="dom" />
// A `WorkerFactory` that runs each Monty worker in a browser Web Worker — the
// production browser backend, whose `terminate()` gives the pool's watchdog its
// hard kill.
//
// Browser-only (uses the global `Worker`); Node uses `nodeWorkerFactory`. Both
// produce a `WorkerChannel`, so the pool is identical. The worker entry is
// resolved relative to this module so a bundler can emit it as a worker chunk;
// pass `workerUrl` to override.

import { WorkerChannel, type WorkerChannelOptions, type WorkerLike } from './channel.js'
import type { WorkerFactory } from './pool.js'

/** Spawns browser Web Workers that serve `module`. */
export function browserWorkerFactory(
  module: WebAssembly.Module,
  options: WorkerChannelOptions = {},
  workerUrl?: string | URL,
): WorkerFactory {
  return () => {
    // The default branch must keep `new Worker(new URL('…', import.meta.url),
    // { type: 'module' })` inline and literal: that exact shape is what
    // Vite/webpack statically detect to emit the worker as its own bundled
    // chunk. Threading the URL through a variable defeats that detection.
    const worker = workerUrl
      ? new Worker(workerUrl, { type: 'module' })
      : new Worker(new URL('./browserWorkerEntry.js', import.meta.url), { type: 'module' })
    worker.postMessage({ init: true, module })
    const like: WorkerLike = {
      post: (message) => worker.postMessage(message),
      onMessage: (handler) => worker.addEventListener('message', (event) => handler(event.data)),
      onError: (handler) => worker.addEventListener('error', (event) => handler(event)),
      terminate: () => worker.terminate(),
    }
    return Promise.resolve(new WorkerChannel(like, options))
  }
}
