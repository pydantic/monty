// The remote pool: `MontyRemote` holds one HTTP/2 connection to a gRPC
// server and opens a session stream per checkout instead of spawning local
// `monty subprocess` children. Dialing, framing, auto-resume after a server
// drain and the per-turn watchdog all live in the native `monty-pool` crate
// (shared with pydantic_monty's AsyncMontyRemote); sessions are the same
// `MontySession` a local pool hands out.

import { NativePool, decodeAsyncCall } from '../native-addon.js'
import { type CheckoutOptions, graceMs, nativeCheckoutOptions } from './pool.js'
import { enterSession, HostModules, MontySession } from './session.js'
import { captureTelemetryContext } from './telemetry.js'

/** Options for [`MontyRemote.create`]. */
export interface MontyRemoteOptions {
  /** Cap on concurrent session streams; checkouts beyond it wait (default: CPU count). */
  maxProcesses?: number
  /**
   * Seconds `checkout()` waits for capacity before rejecting (default: wait
   * forever).
   */
  checkoutTimeout?: number
  /**
   * Hard per-turn deadline in seconds (default 10, `null` disables): a turn
   * that gets no answer in time has its stream ended and fails with
   * `MontyCrashedError` (`timedOut: true`). Without it, a server that admits
   * the stream but never answers blocks the turn until the far end ends it.
   * It also bounds opening the stream, including the server's admission (30s
   * when `null`). `installDependencies` is a turn too, and a real install can exceed 10s.
   * CPU-mode servers require a finite `requestTimeout`.
   */
  requestTimeout?: number | null
  /**
   * Called at each checkout for extra request metadata on that session's
   * stream, e.g. a short-lived auth token: lowercase ASCII names, ASCII
   * values. May return a promise. A resume after a server shutdown reuses
   * the metadata of the original checkout.
   */
  metadata?: () => Record<string, string> | Promise<Record<string, string>>
  /**
   * The former name of `metadata`, accepted for one release; passing both
   * throws a `TypeError`.
   * @deprecated Use `metadata`.
   */
  connectHeaders?: () => Record<string, string> | Promise<Record<string, string>>
  /**
   * Client grace for server wall-time budgets (default 1, `null` disables).
   * CPU mode uses the server's supervisor and grace instead.
   */
  feedDurationLimitGrace?: number | null
  /** As `feedDurationLimitGrace`, for the per-turn budget. */
  turnDurationLimitGrace?: number | null
  /**
   * Resume a session a draining server stored on another stream instead of
   * raising `MontyShutdown` (default `true`). Only a server that stores
   * sessions can be resumed; see `MontyShutdown`.
   */
  autoResume?: boolean
}

/** Options for [`MontyRemote.checkout`]: the local options plus the server's session settings. */
export interface RemoteCheckoutOptions extends CheckoutOptions {
  /**
   * `true` asks a server that stores sessions not to store this one, so it has
   * no `sessionId`; `false` asks it to. Default: the server's choice.
   */
  ephemeral?: boolean
  /** Server-side profile to run the session under, sent unchecked (default: the server's). */
  profile?: string
}

/**
 * An async pool of remote `monty` workers reached over gRPC — the intended
 * peer is `monty-server`. The pool keeps one HTTP/2 connection to `url` and
 * each checkout opens a session stream on it; streams are never pre-warmed
 * or reused. The transport provides no isolation of its own: the server is
 * responsible for sandboxing. Also exported as `MontyWebsocket`, its deprecated former name.
 *
 * ```ts
 * await using pool = await MontyRemote.create('http://127.0.0.1:8000/')
 * await using session = await pool.checkout()
 * const result = await session.feedRun('1 + 1') // 2
 * ```
 */
export class MontyRemote {
  private readonly native: NativePool
  private readonly metadata: MontyRemoteOptions['metadata']
  private closed = false

  private constructor(native: NativePool, metadata: MontyRemoteOptions['metadata']) {
    this.native = native
    this.metadata = metadata
  }

  /**
   * Creates the pool. `url` is the server's `http://` or `https://` URL
   * (`ws://` and `wss://` are read as those two); an `https://` certificate
   * is verified against the operating system's trust store. Nothing is
   * dialed until the first checkout.
   */
  static async create(url: string, options: MontyRemoteOptions = {}): Promise<MontyRemote> {
    if (options.connectHeaders !== undefined && options.metadata !== undefined) {
      throw new TypeError('pass metadata, not connectHeaders as well')
    }
    // `!== undefined`, not `??`: an explicit `metadata: null` is a bad value to
    // reject, not an absent one to fill from the alias
    const [metadata, option] =
      options.metadata !== undefined ? [options.metadata, 'metadata'] : [options.connectHeaders, 'connectHeaders']
    if (metadata !== undefined && typeof metadata !== 'function') {
      throw new TypeError(`${option} must be a function returning a record of header names to values`)
    }
    const requestTimeout = options.requestTimeout === undefined ? 10 : options.requestTimeout
    const native = NativePool.remote({
      url,
      ...(options.maxProcesses !== undefined ? { maxProcesses: options.maxProcesses } : {}),
      ...(options.checkoutTimeout !== undefined ? { checkoutTimeoutMs: options.checkoutTimeout * 1000 } : {}),
      ...(requestTimeout !== null ? { requestTimeoutMs: requestTimeout * 1000 } : {}),
      ...graceMs('feedDurationLimitGraceMs', options.feedDurationLimitGrace),
      ...graceMs('turnDurationLimitGraceMs', options.turnDurationLimitGrace),
      autoResume: options.autoResume ?? true,
    })
    await native.start()
    return new MontyRemote(native, metadata)
  }

  /**
   * Opens a session stream to the server and creates a REPL session there.
   * Rejects with a plain `Error` when the connection fails;
   * `MontyRefusedError` when the server refuses the stream, e.g. one at
   * capacity; `MontyRuntimeError` when the server refuses the session; and
   * `MontyDisconnectError` / `MontyShutdown` when it is lost while being
   * created. Release the stream with `session.close()` (or `await using`).
   */
  async checkout(options: RemoteCheckoutOptions = {}): Promise<MontySession> {
    if (this.closed) {
      throw new Error('the pool is closed — create a new MontyRemote pool')
    }
    const modules = new HostModules(options.externalModules)
    const native = this.native.checkout({
      ...nativeCheckoutOptions(options),
      ...(options.ephemeral !== undefined ? { ephemeral: options.ephemeral } : {}),
      ...(options.profile !== undefined ? { profile: options.profile } : {}),
    })
    // captured before the headers callback can await, as `Monty.checkout` does
    const telemetryContext = captureTelemetryContext()
    const headers = this.metadata === undefined ? [] : headerList(await this.metadata())
    await enterSession(native, telemetryContext, headers)
    return new MontySession(native, modules, decodeAsyncCall)
  }

  /**
   * Shuts the pool down: pending and new checkouts reject. Sessions already
   * checked out keep their streams until closed.
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

/** Validates a `metadata` result into the native header list. */
function headerList(headers: unknown): { name: string; value: string }[] {
  if (typeof headers !== 'object' || headers === null || Array.isArray(headers)) {
    throw new TypeError(`metadata must return a record of header names to values, got ${describe(headers)}`)
  }
  return Object.entries(headers).map(([name, value]) => {
    if (typeof value !== 'string') {
      throw new TypeError(`metadata must return string header values, got ${describe(value)} for '${name}'`)
    }
    return { name, value }
  })
}

/** @deprecated `MontyWebsocket` is the former name of `MontyRemote`; use that instead. */
export const MontyWebsocket = MontyRemote
/** @deprecated `MontyWebsocket` is the former name of `MontyRemote`; use that instead. */
export type MontyWebsocket = MontyRemote
/** @deprecated The former name of `MontyRemoteOptions`; use that instead. */
export type MontyWebsocketOptions = MontyRemoteOptions
/** @deprecated The former name of `RemoteCheckoutOptions`; use that instead. */
export type WebsocketCheckoutOptions = RemoteCheckoutOptions

/** Names a value's type for an error message. */
function describe(value: unknown): string {
  return value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value
}
