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
import type { ComponentModules } from './host.js'
import type { WorkerFactory } from './pool.js'

const entryPath = join(dirname(fileURLToPath(import.meta.url)), 'nodeWorkerEntry.js')

/** Spawns worker threads that instantiate and serve `modules`. */
export function nodeWorkerFactory(modules: ComponentModules, options: WorkerChannelOptions = {}): WorkerFactory {
  return () => {
    const worker = new Worker(entryPath, { workerData: { modules } })
    const like: WorkerLike = {
      post: (message) => worker.postMessage(message),
      onMessage: (handler) => worker.on('message', handler),
      onError: (handler) => worker.on('error', handler),
      terminate: () => void worker.terminate(),
    }
    return Promise.resolve(new WorkerChannel(like, options))
  }
}
