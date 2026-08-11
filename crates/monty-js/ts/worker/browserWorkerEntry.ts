/// <reference lib="webworker" />
// Browser Web Worker entry for a Monty worker.
//
// Runs inside a `Worker`: the host posts an init message carrying the compiled
// wasm module, then dispatch requests; this serves each turn back over
// `postMessage`. The Node `worker_threads` analog is `nodeWorkerEntry.ts`.
//
// Requests can arrive before `serveDispatch` finishes initializing the
// `WasmHost`, so they are queued until the dispatch handler is ready.

import type { DispatchRequest } from './channel.js'
import { serveDispatch } from './serve.js'

/** The init message that delivers the compiled module to the worker. */
interface InitMessage {
  init: true
  module: WebAssembly.Module
}

const queue: DispatchRequest[] = []
let dispatch: ((request: DispatchRequest) => void) | null = null

self.onmessage = (event: MessageEvent<InitMessage | DispatchRequest>) => {
  const data = event.data
  if ('init' in data) {
    void serveDispatch(
      data.module,
      (reply) => self.postMessage(reply),
      (handler) => {
        dispatch = handler
        for (const request of queue) handler(request)
        queue.length = 0
      },
    )
  } else if (dispatch) {
    dispatch(data)
  } else {
    queue.push(data)
  }
}
