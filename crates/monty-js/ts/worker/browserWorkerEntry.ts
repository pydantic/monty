/// <reference lib="webworker" />
// Browser worker entry: initialize once, then acknowledge readiness before serving turns.

import type { DispatchRequest } from './channel.js'
import type { ComponentModules } from './host.js'
import { serveDispatch } from './serve.js'

/** The initial message carrying the compiled component modules. */
interface InitMessage {
  init: true
  modules: ComponentModules
}

self.onmessage = (event: MessageEvent<InitMessage>) => {
  self.onmessage = null
  void serveDispatch(
    event.data.modules,
    (reply) => self.postMessage(reply),
    (handler) => {
      self.onmessage = (request: MessageEvent<DispatchRequest>) => handler(request.data)
    },
  )
}
