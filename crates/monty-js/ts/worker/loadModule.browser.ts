/// <reference lib="dom" />
// Fetches and compiles the core modules generated from the Monty components.

import { CODEC_MODULE_NAMES, COMPONENT_MODULE_NAMES } from './componentModules.js'
import type { ComponentModules } from './host.js'

/** Loads every core module needed to instantiate the bundled component. */
export async function loadModule(): Promise<ComponentModules> {
  // Keep each URL literal so Vite and webpack emit all component assets.
  const modules = await Promise.all([
    WebAssembly.compileStreaming(fetch(new URL('./component/monty.component.core.wasm', import.meta.url))),
    WebAssembly.compileStreaming(fetch(new URL('./component/monty.component.core2.wasm', import.meta.url))),
    WebAssembly.compileStreaming(fetch(new URL('./component/monty.component.core3.wasm', import.meta.url))),
    WebAssembly.compileStreaming(fetch(new URL('./component/monty.component.core4.wasm', import.meta.url))),
  ])
  return Object.fromEntries(COMPONENT_MODULE_NAMES.map((name, index) => [name, modules[index]]))
}

/** Loads the protocol codec's core modules, without the interpreter's. */
export async function loadCodecModules(): Promise<ComponentModules> {
  const modules = await Promise.all([
    WebAssembly.compileStreaming(fetch(new URL('./component/monty-proto.component.core.wasm', import.meta.url))),
    WebAssembly.compileStreaming(fetch(new URL('./component/monty-proto.component.core2.wasm', import.meta.url))),
    WebAssembly.compileStreaming(fetch(new URL('./component/monty-proto.component.core3.wasm', import.meta.url))),
    WebAssembly.compileStreaming(fetch(new URL('./component/monty-proto.component.core4.wasm', import.meta.url))),
  ])
  return Object.fromEntries(CODEC_MODULE_NAMES.map((name, index) => [name, modules[index]]))
}
