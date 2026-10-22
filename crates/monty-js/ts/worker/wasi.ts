// The capability-limited WASI imports every Monty component is instantiated with.

import { WASIShim } from '@bytecodealliance/preview2-shim/instantiation'

/** Creates isolated WASI imports with no host filesystem, environment, or network. */
export function wasiImports(): Record<string, unknown> {
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
