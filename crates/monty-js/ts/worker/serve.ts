// The worker-side dispatch loop, shared by every environment's worker entry.
//
// Runs inside the worker thread/context: owns one component instance and
// answers each `DispatchRequest` by running one turn and posting its semantic
// events back. The environment-specific entry
// (Node `worker_threads`, browser `Worker`) only wires its message primitives
// to `post`/`subscribe`.

import type { DispatchReply, DispatchRequest } from './channel.js'
import { type ComponentModules, WasmHost } from './host.js'

/**
 * Serves turns for one worker until it is terminated. `subscribe` registers the
 * per-request handler; `post` sends each reply back to the channel.
 */
export async function serveDispatch(
  modules: ComponentModules,
  post: (reply: DispatchReply) => void,
  subscribe: (handler: (request: DispatchRequest) => void) => void,
): Promise<void> {
  const host = await WasmHost.create(modules)
  subscribe((request) => {
    const result = host.dispatch(request.request)
    post({ id: request.id, ...result })
  })
}
