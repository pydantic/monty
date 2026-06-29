// Browser entry for `@pydantic/monty/wasm` (the `browser` export condition):
// the base API plus a `createMonty` that fetches + compiles the bundled wasm.

export * from './index.js'

import { loadModule } from './loadModule.browser.js'
import { makeCreateMonty } from './index.js'

/** Fetches + compiles the bundled wasm and returns a ready pool. */
export const createMonty = makeCreateMonty(loadModule)
