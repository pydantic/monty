// Loads the bundled component's core modules from disk under Node.

import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

import { COMPONENT_MODULE_NAMES } from './componentModules.js'
import type { ComponentModules } from './host.js'

/** Reads and compiles every core module needed to instantiate the component. */
export async function loadModule(): Promise<ComponentModules> {
  const entries = await Promise.all(
    COMPONENT_MODULE_NAMES.map(async (name) => {
      const path = fileURLToPath(new URL(`./component/${name}`, import.meta.url))
      const module = await WebAssembly.compile(await readFile(path))
      return [name, module] as const
    }),
  )
  return Object.fromEntries(entries)
}
