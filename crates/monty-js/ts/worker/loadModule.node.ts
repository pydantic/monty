// Loads the bundled components' core modules from disk under Node.

import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

import { CODEC_MODULE_NAMES, COMPONENT_MODULE_NAMES } from './componentModules.js'
import type { ComponentModules } from './host.js'

/** Reads and compiles every core module needed to instantiate the component. */
export function loadModule(): Promise<ComponentModules> {
  return compileAll(COMPONENT_MODULE_NAMES)
}

/** Reads and compiles the protocol codec's core modules, without the interpreter's. */
export function loadCodecModules(): Promise<ComponentModules> {
  return compileAll(CODEC_MODULE_NAMES)
}

/** Compiles the named files from the `component` directory beside this module. */
async function compileAll(names: readonly string[]): Promise<ComponentModules> {
  const entries = await Promise.all(
    names.map(async (name) => {
      const path = fileURLToPath(new URL(`./component/${name}`, import.meta.url))
      const module = await WebAssembly.compile(await readFile(path))
      return [name, module] as const
    }),
  )
  return Object.fromEntries(entries)
}
