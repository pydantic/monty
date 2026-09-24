// Capability-limited component instantiation, used only inside workers.

import { WASIShim } from '@bytecodealliance/preview2-shim/instantiation'

import { instantiate } from './component/monty.component.js'
import type {
  DispatchResult as ComponentDispatchResult,
  Request as ComponentRequest,
} from './component/monty.component.js'

/** Core modules emitted by Jco for one transpiled WebAssembly component. */
export type ComponentModules = Readonly<Record<string, WebAssembly.Module>>

/** Semantic request accepted by the Rust component. */
export type DispatchRequest = ComponentRequest

/** Component output plus termination diagnostics supplied by the host runtime. */
export interface DispatchResult extends ComponentDispatchResult {
  exitStatus?: string | null
}

/** Sends one semantic request, optionally bounded by a duration backstop. */
export type Dispatcher = (request: DispatchRequest, timeoutMs?: number) => Promise<DispatchResult>

/** Instantiates an isolated component and returns its persistent turn dispatcher. */
export async function instantiateWorker(
  modules: ComponentModules,
): Promise<(request: DispatchRequest) => DispatchResult> {
  const component = await instantiate((path) => getModule(modules, path), wasiImports())
  return component.worker.dispatch
}

/** Creates isolated WASI imports with no host filesystem, environment, or network. */
function wasiImports(): Record<string, unknown> {
  const imports = new WASIShim({ sandbox: { preopens: {}, env: {}, args: [] } }).getImportObject()
  return {
    'wasi:cli/environment': imports['wasi:cli/environment'],
    'wasi:cli/exit': {
      exit: denyProcessExit,
      exitWithCode: denyProcessExit,
    },
    'wasi:cli/stderr': imports['wasi:cli/stderr'],
    'wasi:cli/stdin': imports['wasi:cli/stdin'],
    'wasi:cli/stdout': imports['wasi:cli/stdout'],
    'wasi:clocks/monotonic-clock': imports['wasi:clocks/monotonic-clock'],
    'wasi:clocks/wall-clock': imports['wasi:clocks/wall-clock'],
    'wasi:filesystem/preopens': imports['wasi:filesystem/preopens'],
    'wasi:filesystem/types': imports['wasi:filesystem/types'],
    'wasi:io/error': imports['wasi:io/error'],
    'wasi:io/streams': imports['wasi:io/streams'],
    'wasi:random/random': imports['wasi:random/random'],
  }
}

/** Turns a guest exit into a component failure instead of terminating Node. */
function denyProcessExit(): never {
  throw new Error('Monty wasm component requested process exit')
}

/** Resolves Jco's relative core-module path against the precompiled module map. */
function getModule(modules: ComponentModules, path: string): WebAssembly.Module {
  const module = modules[path] ?? modules[path.replace(/^\.\//, '')]
  if (!module) throw new Error(`component core module is missing: ${path}`)
  return module
}
