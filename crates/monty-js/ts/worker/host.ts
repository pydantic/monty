// Loads the lean wasip1 worker module under a single-threaded WASI shim and
// runs it one protocol turn at a time.
//
// The module (`crates/monty-wasm`) is a WASI reactor exporting
// `monty_dispatch_turn`: it reads one framed request from stdin and writes the
// turn's framed events to stdout. This host owns the instance (so session state
// persists across turns) and swaps the stdin/stdout buffers around each call.
//
// In Node this drives the module directly; in a browser the same code runs
// inside a Web Worker. Nothing here is napi- or pool-specific.

import { File, OpenFile, WASI } from '@bjorn3/browser_wasi_shim'

/** Status codes returned by the module's `monty_dispatch_turn` export. */
export const TurnStatus = {
  Continue: 0,
  Shutdown: 1,
  IoError: 2,
} as const

interface WasmExports {
  monty_dispatch_turn(): number
}

/**
 * Sends one framed request and resolves to the turn's framed reply. The
 * abstraction `WorkerTransport` drives, so the same transport works over an
 * in-process [`WasmHost`] (see [`inProcessDispatcher`]) and over a Web Worker
 * `postMessage` channel.
 */
export type Dispatcher = (requestFrame: Uint8Array) => Promise<{ reply: Uint8Array; status: number }>

/** Adapts a synchronous in-process [`WasmHost`] to the async [`Dispatcher`]. */
export function inProcessDispatcher(host: WasmHost): Dispatcher {
  return (requestFrame) => Promise.resolve(host.dispatch(requestFrame))
}

/** One instantiated worker module; reused across turns. */
export class WasmHost {
  private constructor(
    private readonly wasi: WASI,
    private readonly exports: WasmExports,
  ) {}

  /** Instantiates the module and runs its WASI reactor initializer. */
  static async create(module: WebAssembly.Module): Promise<WasmHost> {
    const wasi = new WASI([], [], [stdio(), stdio(), stdio()])
    const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport })
    // browser_wasi_shim types `initialize` against its own narrower instance
    // shape; a core `WebAssembly.Instance` satisfies it structurally.
    wasi.initialize(instance as unknown as Parameters<WASI['initialize']>[0])
    return new WasmHost(wasi, instance.exports as unknown as WasmExports)
  }

  /**
   * Runs one turn: feeds `requestFrame` on stdin, returns the concatenated
   * framed reply events from stdout and the turn's status code.
   */
  dispatch(requestFrame: Uint8Array): { reply: Uint8Array; status: number } {
    this.wasi.fds[0] = new OpenFile(new File(Array.from(requestFrame)))
    const out = new File([])
    this.wasi.fds[1] = new OpenFile(out)
    const status = this.exports.monty_dispatch_turn()
    return { reply: out.data, status }
  }
}

function stdio(): OpenFile {
  return new OpenFile(new File([]))
}
