// The worker-side dispatch loop, shared by every environment's worker entry.
//
// Runs inside the worker thread/context: compiles nothing itself, but owns the
// one `WasmHost` instance and answers each `DispatchRequest` by running one
// turn and posting the framed reply back. The environment-specific entry
// (Node `worker_threads`, browser `Worker`) only wires its message primitives
// to `post`/`subscribe`.

import type { DispatchReply, DispatchRequest } from './channel.js'
import { WasmHost } from './host.js'

/**
 * Serves turns for one worker until it is terminated. `subscribe` registers the
 * per-request handler; `post` sends each reply back to the channel.
 */
export async function serveDispatch(
  module: WebAssembly.Module,
  post: (reply: DispatchReply) => void,
  subscribe: (handler: (request: DispatchRequest) => void) => void,
): Promise<void> {
  const host = await WasmHost.create(module)
  subscribe((request) => {
    const { reply, status, events } = host.dispatch(request.frame)
    post({ id: request.id, reply, status, events })
  })
}
