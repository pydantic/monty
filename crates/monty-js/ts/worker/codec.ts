// The protocol codec component, instantiated on the calling thread: it holds
// no interpreter and no state, so unlike the worker component it needs no
// Web Worker to isolate or preempt it.

import type { DecodedFrame, Request } from './component/monty-codec.component.js'
import { getModule } from './componentModules.js'
import type { ComponentModules } from './host.js'
import { wasiImports } from './wasi.js'

/** Converts semantic requests and events to and from protobuf frames. */
export interface Codec {
  /** Encodes one request; throws `CodecError` when the protocol rejects it. */
  encodeRequest(request: Request): Uint8Array
  /** Validates and decodes one frame from the remote; throws `CodecError` on a bad one. */
  decodeEvent(frame: Uint8Array): DecodedFrame
}

/** A request the codec refused to encode, or a frame it refused to decode. */
export class CodecError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'CodecError'
  }
}

/** Instantiates the codec from its precompiled core modules. */
export async function instantiateCodec(modules: ComponentModules): Promise<Codec> {
  // imported here, not at the top: the bindings exist only after `build:wasm`,
  // and the entries that re-export `MontyWebsocket` must load without them
  const { instantiate } = await import('./component/monty-codec.component.js')
  const { codec } = await instantiate((path) => getModule(modules, path), wasiImports())
  return {
    encodeRequest: (request) => unwrap(() => codec.encodeRequest(request)),
    decodeEvent: (frame) => unwrap(() => codec.decodeEvent(frame)),
  }
}

/** Runs one codec call, turning the component's `result` error into a `CodecError`. */
function unwrap<T>(call: () => T): T {
  try {
    return call()
  } catch (error) {
    // Jco throws a WIT `result` error as a ComponentError whose `payload` is the value
    const payload = (error as { payload?: unknown }).payload
    if (typeof payload === 'string') throw new CodecError(payload)
    throw error
  }
}
