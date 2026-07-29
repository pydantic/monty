// Loads the lean wasip1 worker module under a single-threaded WASI shim and
// runs it one protocol turn at a time.
//
// The module (`crates/monty-wasm-runtime`) is a WASI reactor exporting
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
  monty_decode_child_events(): number
}

/**
 * Sends one framed request and resolves to the turn's framed reply. The
 * abstraction `WorkerTransport` drives, so the same transport works over an
 * in-process [`WasmHost`] (see [`inProcessDispatcher`]) and over a Web Worker
 * `postMessage` channel.
 */
export type Dispatcher = (
  requestFrame: Uint8Array,
) => Promise<{ reply: Uint8Array; status: number; events?: DecodedChildEvent[] }>

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
  dispatch(requestFrame: Uint8Array): { reply: Uint8Array; status: number; events: DecodedChildEvent[] } {
    const { output: reply, status } = this.callWithStdio(requestFrame, () => this.exports.monty_dispatch_turn())
    const events = status === TurnStatus.IoError ? [] : this.decodeChildEvents(reply)
    return { reply, status, events }
  }

  /** Decodes framed `ChildEvent`s with the worker module's Rust protobuf code. */
  decodeChildEvents(reply: Uint8Array): DecodedChildEvent[] {
    const { output, status } = this.callWithStdio(reply, () => this.exports.monty_decode_child_events())
    if (status !== TurnStatus.Continue) {
      throw new Error('failed to decode wasm worker reply')
    }
    return JSON.parse(new TextDecoder().decode(output)) as DecodedChildEvent[]
  }

  private callWithStdio(input: Uint8Array, call: () => number): { output: Uint8Array; status: number } {
    this.wasi.fds[0] = new OpenFile(new File(Array.from(input)))
    const out = new File([])
    this.wasi.fds[1] = new OpenFile(out)
    const status = call()
    return { output: out.data, status }
  }
}

export interface DecodedChildEvent {
  kind: number
  bytes: number[]
}

function stdio(): OpenFile {
  return new OpenFile(new File([]))
}
