import type { WorkerMessage, DispatchRequest } from './channel.js'
import { type ComponentModules, instantiateWorker } from './host.js'

/** Initializes one component, acknowledges readiness, then serves turns until termination. */
export async function serveDispatch(
  modules: ComponentModules,
  post: (reply: WorkerMessage) => void,
  subscribe: (handler: (request: DispatchRequest) => void) => void,
): Promise<void> {
  try {
    const dispatch = await instantiateWorker(modules)
    subscribe(({ id, request }) => post({ id, ...dispatch(request) }))
    post({ ready: true })
  } catch (error) {
    post({ startupError: error instanceof Error ? error.message : String(error) })
  }
}
