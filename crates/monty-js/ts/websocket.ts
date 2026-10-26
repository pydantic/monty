// The remote pool: `MontyWebsocket` dials a WebSocket server per checkout
// instead of spawning local `monty subprocess` children. Dialing, framing,
// auto-resume after a server drain and the per-turn watchdog all live in the
// native `monty-pool` crate (shared with pydantic_monty's AsyncMontyWebsocket);
// sessions are the same `MontySession` a local pool hands out.

import { NativePool } from '../native-addon.js'
import { type CheckoutOptions, graceMs, nativeCheckoutOptions } from './pool.js'
import { enterSession, HostModules, MontySession } from './session.js'
import { captureTelemetryContext } from './telemetry.js'

/** Options for [`MontyWebsocket.create`]. */
export interface MontyWebsocketOptions {
  /** Cap on concurrent connections; checkouts beyond it wait (default: CPU count). */
  maxProcesses?: number
  /**
   * Seconds `checkout()` waits for capacity before rejecting (default: wait
   * forever).
   */
  checkoutTimeout?: number
  /**
   * Hard per-turn deadline in seconds (default 10, `null` disables): a turn
   * that gets no answer in time has its connection killed and fails with
   * `MontyCrashedError` (`timedOut: true`). Without it, a server that accepts
   * the connection but never answers blocks the turn until the far end closes.
   * `installDependencies` is a turn too, and a real install can exceed 10s.
   */
  requestTimeout?: number | null
  /**
   * Called at each checkout for extra headers on that connection's upgrade
   * request, e.g. a short-lived auth token. May return a promise. A resume
   * after a server shutdown reuses the headers of the original checkout.
   */
  connectHeaders?: () => Record<string, string> | Promise<Record<string, string>>
  /** As [`MontyOptions.feedDurationLimitGrace`] (default 1, `null` disables). */
  feedDurationLimitGrace?: number | null
  /** As [`MontyOptions.turnDurationLimitGrace`] (default 1, `null` disables). */
  turnDurationLimitGrace?: number | null
  /**
   * When a server that stores sessions drains, redial, load the state it named
   * into a new session and re-send the request, so the caller sees the result
   * rather than `MontyShutdown` (default true).
   */
  autoResume?: boolean
}

/** Options for [`MontyWebsocket.checkout`]: the local options plus the server's session settings. */
export interface WebsocketCheckoutOptions extends CheckoutOptions {
  /**
   * `true` asks a server that stores sessions not to store this one, so it has
   * no `sessionId`; `false` asks it to. Default: the server's choice.
   */
  ephemeral?: boolean
  /** Server-side profile to run the session under, sent unchecked (default: the server's). */
  profile?: string
}

/**
 * An async pool of remote `monty` workers reached over a WebSocket — the
 * intended peer is `monty-server`. Each checkout dials `url`; connections are
 * never pre-warmed or reused. The transport provides no isolation of its own:
 * the server is responsible for sandboxing.
 *
 * ```ts
 * await using pool = await MontyWebsocket.create('ws://127.0.0.1:8000/')
 * await using session = await pool.checkout()
 * const result = await session.feedRun('1 + 1') // 2
 * ```
 */
export class MontyWebsocket {
  private readonly native: NativePool
  private readonly connectHeaders: MontyWebsocketOptions['connectHeaders']
  private closed = false

  private constructor(native: NativePool, connectHeaders: MontyWebsocketOptions['connectHeaders']) {
    this.native = native
    this.connectHeaders = connectHeaders
  }

  /**
   * Creates the pool. `url` is a `ws://` or `wss://` URL dialed verbatim, so
   * any routing the server needs must already be in it. Nothing is dialed
   * until the first checkout.
   */
  static async create(url: string, options: MontyWebsocketOptions = {}): Promise<MontyWebsocket> {
    if (options.connectHeaders !== undefined && typeof options.connectHeaders !== 'function') {
      throw new TypeError('connectHeaders must be a function returning a record of header names to values')
    }
    const requestTimeout = options.requestTimeout === undefined ? 10 : options.requestTimeout
    const native = NativePool.websocket({
      url,
      ...(options.maxProcesses !== undefined ? { maxProcesses: options.maxProcesses } : {}),
      ...(options.checkoutTimeout !== undefined ? { checkoutTimeoutMs: options.checkoutTimeout * 1000 } : {}),
      ...(requestTimeout !== null ? { requestTimeoutMs: requestTimeout * 1000 } : {}),
      ...graceMs('feedDurationLimitGraceMs', options.feedDurationLimitGrace),
      ...graceMs('turnDurationLimitGraceMs', options.turnDurationLimitGrace),
      autoResume: options.autoResume ?? true,
    })
    await native.start()
    return new MontyWebsocket(native, options.connectHeaders)
  }

  /**
   * Dials the server and creates a REPL session there. Rejects with a plain
   * `Error` when the connection or upgrade fails, e.g. a server at capacity;
   * `MontyRuntimeError` when the server refuses the session; and
   * `MontyDisconnectError` / `MontyShutdown` when it is lost while being created.
   * Release the connection with `session.close()` (or `await using`).
   */
  async checkout(options: WebsocketCheckoutOptions = {}): Promise<MontySession> {
    if (this.closed) {
      throw new Error('the pool is closed — create a new MontyWebsocket pool')
    }
    const modules = new HostModules(options.externalModules)
    const native = this.native.checkout({
      ...nativeCheckoutOptions(options),
      ...(options.ephemeral !== undefined ? { ephemeral: options.ephemeral } : {}),
      ...(options.profile !== undefined ? { profile: options.profile } : {}),
    })
    // captured before the headers callback can await, as `Monty.checkout` does
    const telemetryContext = captureTelemetryContext()
    const headers = this.connectHeaders === undefined ? [] : headerList(await this.connectHeaders())
    await enterSession(native, telemetryContext, headers)
    return new MontySession(native, modules)
  }

  /**
   * Shuts the pool down: pending and new checkouts reject. Sessions already
   * checked out keep their connections until closed.
   */
  async close(): Promise<void> {
    if (this.closed) {
      return
    }
    this.closed = true
    await this.native.close()
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }
}

/** Validates a `connectHeaders` result into the native header list. */
function headerList(headers: unknown): { name: string; value: string }[] {
  if (typeof headers !== 'object' || headers === null || Array.isArray(headers)) {
    throw new TypeError(`connectHeaders must return a record of header names to values, got ${describe(headers)}`)
  }
  return Object.entries(headers).map(([name, value]) => {
    if (typeof value !== 'string') {
      throw new TypeError(`connectHeaders must return string header values, got ${describe(value)} for '${name}'`)
    }
    return { name, value }
  })
}

/** Names a value's type for an error message. */
function describe(value: unknown): string {
  return value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value
}
