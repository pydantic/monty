// Node entry for `@pydantic/monty/wasm` (the `node` export condition): the base
// API plus a `createMonty` that auto-loads the wasm from disk.

export * from './index.js'

import { loadModule } from './loadModule.node.js'
import { makeCreateMonty } from './index.js'

/** Loads the bundled wasm from disk and returns a ready pool. */
export const createMonty = makeCreateMonty(loadModule)
