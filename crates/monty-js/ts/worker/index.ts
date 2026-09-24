// Browser/default WASM entry. Node selects index.node.ts through package exports.
import { browserWorkerFactory } from './browserFactory.js'
import type { ComponentModules } from './host.js'
import { loadModule } from './loadModule.browser.js'
import {
  createWorkerPoolFromFactory,
  workerChannelOptions,
  type Monty as MontyPool,
  type WasmPoolOptions,
} from './poolOptions.js'

export * from '../shared.js'
export { loadModule }
export type { ComponentModules } from './host.js'
export type { WasmPoolOptions, WasmPoolOptions as MontyOptions } from './poolOptions.js'
export type Monty = MontyPool

/** Creates a Web Worker pool from precompiled modules; there is no in-process fallback. */
export async function createWorkerPool(modules: ComponentModules, options: WasmPoolOptions = {}): Promise<Monty> {
  if (typeof Worker === 'undefined') {
    throw new Error('Monty requires Web Workers; in Node, import @pydantic/monty/wasm using the node export condition')
  }
  const factory = browserWorkerFactory(modules, workerChannelOptions(options), options.workerUrl)
  return createWorkerPoolFromFactory(factory, options, globalThis.navigator?.hardwareConcurrency || 4)
}

/** Loads the bundled component into hard-preemptible Web Workers. */
export const Monty = {
  async create(options: WasmPoolOptions = {}): Promise<Monty> {
    return createWorkerPool(await loadModule(), options)
  },
}
