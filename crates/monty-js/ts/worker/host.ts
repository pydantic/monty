// Capability-limited component instantiation, used only inside workers.

import { getModule } from './componentModules.js'
import { instantiate } from './component/monty.component.js'
import type { DispatchRequest, DispatchResult } from './dispatch.js'
import { wasiImports } from './wasi.js'

/** Core modules emitted by Jco for one transpiled WebAssembly component. */
export type ComponentModules = Readonly<Record<string, WebAssembly.Module>>

/** Instantiates an isolated component and returns its persistent turn dispatcher. */
export async function instantiateWorker(
  modules: ComponentModules,
): Promise<(request: DispatchRequest) => DispatchResult> {
  const component = await instantiate((path) => getModule(modules, path), wasiImports())
  return component.worker.dispatch
}
