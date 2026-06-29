// A `WorkerFactory` that runs each Monty worker in a Node `worker_threads`
// thread — the Node analog of the browser's Web Worker backend, and the one
// the pool's watchdog can hard-kill.
//
// Node-only (imports `node:worker_threads`); browsers use a `Worker`-based
// factory instead. Both produce a `WorkerChannel`, so the pool is identical.

import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { Worker } from 'node:worker_threads'

import { WorkerChannel, type WorkerChannelOptions, type WorkerLike } from './channel.js'
import type { WorkerFactory } from './pool.js'

const entryPath = join(dirname(fileURLToPath(import.meta.url)), 'nodeWorkerEntry.ts')

/** Spawns workers that serve `module` in a worker thread. */
export function nodeWorkerFactory(module: WebAssembly.Module, options: WorkerChannelOptions = {}): WorkerFactory {
  return () => {
    const worker = new Worker(entryPath, {
      // run the .ts entry through the same loader the tests use
      execArgv: ['--import', '@oxc-node/core/register'],
      workerData: { module },
    })
    const like: WorkerLike = {
      post: (message) => worker.postMessage(message),
      onMessage: (handler) => worker.on('message', handler),
      onError: (handler) => worker.on('error', handler),
      terminate: () => void worker.terminate(),
    }
    return Promise.resolve(new WorkerChannel(like, options))
  }
}
