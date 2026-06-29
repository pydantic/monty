// Node `worker_threads` entry for a Monty worker.
//
// Runs in the worker thread: serves dispatch turns over `parentPort` using the
// already-compiled wasm module passed via `workerData` (a `WebAssembly.Module`
// is structured-cloneable across threads, so no per-worker recompile). The
// browser equivalent is the same against `self.postMessage` / `self.onmessage`.

import { parentPort, workerData } from 'node:worker_threads'

import { serveDispatch } from './serve.js'

void (async () => {
  const port = parentPort
  if (!port) throw new Error('nodeWorkerEntry must run as a worker thread')
  const { module } = workerData as { module: WebAssembly.Module }
  await serveDispatch(
    module,
    (reply) => port.postMessage(reply),
    (handler) => port.on('message', handler),
  )
})()
