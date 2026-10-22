// The remote pool of the WebAssembly entries: `MontyWebsocket` dials a WebSocket
// server per checkout through the runtime's global `WebSocket`, so it works in
// browsers (and Node 22+) where the native `MontyWebsocket` cannot load. Its
// sessions are the `MontySession` the wasm pool returns, driven by
// `WorkerTransport` over a `RemoteDispatcher`; only the codec component is
// loaded, never the interpreter.

import { HostModules, MontySession } from '../session.js'
import type {
  MontyWebsocket as NativeMontyWebsocket,
  MontyWebsocketOptions,
  WebsocketCheckoutOptions,
} from '../websocket.js'
import { type Codec, instantiateCodec } from './codec.js'
import type { ConfigureRequest } from './component/monty-codec.component.js'
import { timeoutOption } from './deadline.js'
import type { ComponentModules } from './host.js'
import { RemoteDispatcher, type RemoteOptions } from './remote.js'
import { type DurationGraces, prepareSession, WorkerTransport } from './transport.js'

type SessionNative = ConstructorParameters<typeof MontySession>[0]

/** The public pool methods, without the native implementation. */
export type MontyWebsocket = Pick<NativeMontyWebsocket, keyof NativeMontyWebsocket>

/** Options for the WebAssembly `MontyWebsocket.create()`: the native ones a browser can honour. */
export type WasmWebsocketOptions = Pick<
  MontyWebsocketOptions,
  'requestTimeout' | 'feedDurationLimitGrace' | 'turnDurationLimitGrace' | 'autoResume'
>

/** Native options with no browser equivalent, rejected rather than ignored when set. */
const NATIVE_ONLY: Record<string, string> = {
  connectHeaders: 'a browser WebSocket cannot set upgrade headers',
  maxProcesses: 'it has no connection cap; each checkout dials at once',
  checkoutTimeout: 'it has no connection cap; each checkout dials at once',
}

/** Creates a remote pool whose codec is compiled from `codecModules`. */
export async function createWebsocketPool(
  codecModules: ComponentModules,
  url: string,
  options: WasmWebsocketOptions = {},
): Promise<MontyWebsocket> {
  for (const [option, reason] of Object.entries(NATIVE_ONLY)) {
    // an option left `undefined` is unset, so options shared with the native client still pass
    if ((options as Record<string, unknown>)[option] !== undefined)
      throw new TypeError(`${option} is not supported by the WebAssembly MontyWebsocket: ${reason}`)
  }
  checkUrl(url)
  if (typeof WebSocket === 'undefined') {
    throw new Error('MontyWebsocket needs a global WebSocket, as browsers and Node 22+ provide')
  }
  const requestTimeout = options.requestTimeout === undefined ? 10 : options.requestTimeout
  const remote: RemoteOptions = {
    url,
    ...(requestTimeout === null ? {} : { requestTimeoutMs: seconds(requestTimeout, 'requestTimeout') }),
    autoResume: options.autoResume ?? true,
  }
  const graces: DurationGraces = {
    feedDurationLimitGraceMs: grace(options.feedDurationLimitGrace, 'feedDurationLimitGrace'),
    turnDurationLimitGraceMs: grace(options.turnDurationLimitGrace, 'turnDurationLimitGrace'),
  }
  return new WebsocketPool(await instantiateCodec(codecModules), remote, graces)
}

/** Dials a fresh connection per checkout; connections are never reused. */
class WebsocketPool implements MontyWebsocket {
  private closed = false
  private checkouts = 0

  constructor(
    private readonly codec: Codec,
    private readonly remote: RemoteOptions,
    private readonly graces: DurationGraces,
  ) {}

  /**
   * Dials the server and creates a REPL session there. Rejects when the
   * connection or upgrade fails, e.g. a server at capacity. Release the
   * connection with `session.close()` (or `await using`).
   */
  async checkout(options: WebsocketCheckoutOptions = {}): Promise<MontySession> {
    if (this.closed) throw new Error('the pool is closed — create a new MontyWebsocket pool')
    const config: ConfigureRequest = {
      ...prepareSession(options),
      ephemeral: options.ephemeral,
      profile: options.profile,
    }
    const modules = new HostModules(options.externalModules)
    const dispatcher = await RemoteDispatcher.connect(this.codec, this.remote)
    let transport: WorkerTransport
    try {
      transport = await WorkerTransport.configure(dispatcher.dispatch, config, this.graces, ++this.checkouts, true)
    } catch (error) {
      dispatcher.close()
      throw error
    }
    transport.onFinish = () => dispatcher.close()
    return new MontySession(transport as unknown as SessionNative, modules)
  }

  /** Rejects later checkouts; sessions already checked out keep their connections until closed. */
  async close(): Promise<void> {
    this.closed = true
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }
}

/** Rejects anything but a `ws://` or `wss://` URL before the first dial. */
function checkUrl(url: string): void {
  let protocol: string
  try {
    protocol = new URL(url).protocol
  } catch {
    throw new TypeError(`MontyWebsocket url must be a ws:// or wss:// URL, got ${JSON.stringify(url)}`)
  }
  if (protocol !== 'ws:' && protocol !== 'wss:') {
    throw new TypeError(`MontyWebsocket url must be a ws:// or wss:// URL, got ${JSON.stringify(url)}`)
  }
}

/** A duration-backstop grace in milliseconds: absent means 1s, `null` disables it. */
function grace(value: number | null | undefined, name: string): number | null {
  return value === null ? null : seconds(value ?? 1, name)
}

/** Converts public seconds to validated timer milliseconds. */
function seconds(value: number, name: string): number {
  return timeoutOption(value * 1000, name)!
}
