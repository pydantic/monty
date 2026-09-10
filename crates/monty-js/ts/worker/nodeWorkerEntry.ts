// Node `worker_threads` entry for a Monty worker.
//
// Runs in the worker thread and instantiates the precompiled component modules
// passed through `workerData`. `WebAssembly.Module`s are structured-cloneable,
// so each worker gets isolated state without recompiling. The browser entry
// provides the same transport over `self.postMessage`.

import { parentPort, workerData } from 'node:worker_threads'

import type { ComponentModules } from './host.js'
import { serveDispatch } from './serve.js'

void (async () => {
  const port = parentPort
  if (!port) throw new Error('nodeWorkerEntry must run as a worker thread')
  const { modules } = workerData as { modules: ComponentModules }
  await serveDispatch(
    modules,
    (reply) => port.postMessage(reply),
    (handler) => port.on('message', handler),
  )
})()
