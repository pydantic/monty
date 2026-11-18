import { Monty, type WasmPoolOptions } from '../dist/worker/index.js'
import { Monty as NodeMonty } from '../dist/worker/index.node.js'

// Node's types omit WebAssembly.Module. Supply only the existing WASM API requirement,
// whose interface is empty in TypeScript's DOM library, without adding browser globals.
declare global {
  namespace WebAssembly {
    interface Module {}
  }
}

const options: WasmPoolOptions = { maxProcesses: 1 }
void Monty.create(options)
void NodeMonty.create(options)

// This check must not acquire DOM types through a transitive declaration reference.
// @ts-expect-error Node does not have a global browser Worker.
void Worker
