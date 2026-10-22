// Node WASM entry: filesystem asset loading and worker_threads, never browser imports.
import { availableParallelism } from 'node:os'

import type { ComponentModules } from './host.js'
import { loadCodecModules, loadModule } from './loadModule.node.js'
import { nodeWorkerFactory } from './nodeFactory.js'
import {
  createWorkerPoolFromFactory,
  workerChannelOptions,
  type Monty as MontyPool,
  type WasmPoolOptions,
} from './poolOptions.js'
import {
  createWebsocketPool,
  type MontyWebsocket as MontyWebsocketPool,
  type WasmWebsocketOptions,
} from './websocket.js'

export * from '../shared.js'
export type { WebsocketCheckoutOptions } from '../websocket.js'
export type { WasmWebsocketOptions, WasmWebsocketOptions as MontyWebsocketOptions } from './websocket.js'
export { loadModule }
export type { ComponentModules } from './host.js'
export type { WasmPoolOptions, WasmPoolOptions as MontyOptions } from './poolOptions.js'
export type Monty = MontyPool

/** Creates a hard-preemptible Node worker-thread pool for precompiled modules. */
export async function createWorkerPool(modules: ComponentModules, options: WasmPoolOptions = {}): Promise<Monty> {
  return createWorkerPoolFromFactory(
    nodeWorkerFactory(modules, workerChannelOptions(options)),
    options,
    availableParallelism(),
  )
}

/** Loads the bundled component into hard-preemptible Node worker threads. */
export const Monty = {
  async create(options: WasmPoolOptions = {}): Promise<Monty> {
    return createWorkerPool(await loadModule(), options)
  },
}

export type MontyWebsocket = MontyWebsocketPool

/** Dials remote workers over the global `WebSocket`, loading only the protocol codec. */
export const MontyWebsocket = {
  async create(url: string, options: WasmWebsocketOptions = {}): Promise<MontyWebsocket> {
    return createWebsocketPool(await loadCodecModules(), url, options)
  },
}
