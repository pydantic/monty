// A remote worker as a `Dispatcher`: monty-proto frames over a WebSocket instead
// of requests posted to a local component, so `WorkerTransport` drives a remote
// session exactly as it drives a wasm worker. The codec component does the
// protobuf; this file owns the socket, the per-turn deadline, streamed prints,
// the session ID and the resume after a draining server parks the session,
// mirroring `monty-pool`'s WebSocket checkout (crates/monty-pool/src/checkout.rs).

import { type Codec, CodecError } from './codec.js'
import type { ConfigureRequest, DecodedFrame, Event, Request } from './component/monty-codec.component.js'
import { deadlineTimer, formatDuration } from './deadline.js'
import { DispatchFailure, type DispatchResult, type Dispatcher } from './dispatch.js'

/** How a remote dispatcher reaches and bounds its server. */
export interface RemoteOptions {
  /** A `ws://` or `wss://` URL, dialed verbatim. */
  url: string
  /** Per-turn deadline in milliseconds; also bounds the dial. Absent waits forever. */
  requestTimeoutMs?: number
  /** Resume a session a storing server parks on drain (see `MontyWebsocketOptions.autoResume`). */
  autoResume: boolean
}

type OnPrint = (stream: 'stdout' | 'stderr', text: string) => void

/**
 * One remote session's connection, as the `dispatch` a `WorkerTransport` drives.
 * Single-use, as monty-pool's WebSocket workers are: close it when the session ends.
 */
export class RemoteDispatcher {
  /** The session a storing server named on its last `configure` or `load` reply. */
  private sessionId: Uint8Array | undefined
  /** The session's `configure`, replayed on the new connection of a resume. */
  private configured: ConfigureRequest | null = null
  /** The suspension awaiting an answer, recognised when a resume re-announces it. */
  private pending: PendingKey | null = null

  private constructor(
    private readonly codec: Codec,
    private readonly options: RemoteOptions,
    private link: Link,
  ) {}

  /** Dials the server; rejects when the connection or upgrade fails. */
  static async connect(codec: Codec, options: RemoteOptions): Promise<RemoteDispatcher> {
    return new RemoteDispatcher(codec, options, await Link.open(options))
  }

  readonly dispatch: Dispatcher = (request, backstopMs, onPrint) =>
    this.turn(request, backstopMs, onPrint, this.options.autoResume)

  /** Closes the connection, which is what ends the session on the server. */
  close(): void {
    this.link.close()
  }

  /**
   * Sends one request and reads frames up to its turn-ending event, streaming
   * prints. A `shutdown` naming what to load is resumed when `mayResume`;
   * a lost or misbehaving connection throws `DispatchFailure`.
   */
  private async turn(
    request: Request,
    backstopMs: number | undefined,
    onPrint: OnPrint | undefined,
    mayResume: boolean,
  ): Promise<DispatchResult> {
    let frame: Uint8Array
    try {
      frame = this.codec.encodeRequest(request)
    } catch (error) {
      // never sent, so the server is still in step: a session-preserving error
      if (error instanceof CodecError) return refused(error.message)
      throw error
    }
    if (request.tag === 'configure') this.configured = request.val
    const timeoutMs = Math.min(this.options.requestTimeoutMs ?? Infinity, backstopMs ?? Infinity)
    const reply = await this.link.exchange(frame, timeoutMs, (frame) => this.decode(frame), onPrint)
    this.track(request, reply.event)
    if (reply.event.tag === 'fatal-error' || reply.event.tag === 'shutdown') this.link.close()
    if (reply.event.tag === 'shutdown' && reply.event.val !== undefined && mayResume && this.canResume(request)) {
      const resumed = await this.resume(reply.event.val, request, backstopMs, onPrint)
      if (resumed !== null) return resumed
    }
    return this.result(reply)
  }

  /** Decodes one frame, adopting the session ID it names. */
  private decode(frame: Uint8Array): DecodedFrame {
    const decoded = this.codec.decodeEvent(frame)
    if (decoded.sessionId !== undefined) this.sessionId = decoded.sessionId
    return decoded
  }

  /** Follows the pending suspension as `monty-pool`'s `Checkout::pending` does. */
  private track(request: Request, event: Event): void {
    switch (event.tag) {
      case 'function-call':
      case 'os-call':
        this.pending = { kind: 'call', callId: event.val.callId, functionName: event.val.functionName }
        break
      case 'name-lookup':
        this.pending = { kind: 'lookup', name: event.val.name, objectId: event.val.objectId ?? null }
        break
      case 'resolve-futures':
        this.pending = { kind: 'futures', callIds: [...event.val.pendingCallIds] }
        break
      case 'complete':
      case 'typing-error':
        this.pending = null
        break
      case 'error':
        // a refused dump leaves the feed suspended and resumable
        if (request.tag !== 'dump') this.pending = null
        break
    }
  }

  /** Whether a shutdown answering `request` may be resumed: the server named the
   *  session, and `request` is not session setup or teardown. */
  private canResume(request: Request): boolean {
    return (
      this.sessionId !== undefined &&
      this.configured !== null &&
      request.tag !== 'configure' &&
      request.tag !== 'load' &&
      request.tag !== 'reset'
    )
  }

  /**
   * Reloads a parked session from `state` on a new connection and re-sends
   * `request`, which the server reported it did not run. The `load` reply must
   * match the session at the shutdown: `ok` when idle, or the same suspension
   * re-announced. Returns `null` when reloading fails, so the caller reports the
   * original shutdown; the turns made here never resume again.
   */
  private async resume(
    state: Uint8Array,
    request: Request,
    backstopMs: number | undefined,
    onPrint: OnPrint | undefined,
  ): Promise<DispatchResult | null> {
    const expected = this.pending
    try {
      this.link = await Link.open(this.options)
      const configured = await this.turn({ tag: 'configure', val: this.configured! }, undefined, undefined, false)
      const loaded =
        configured.events[0]?.tag === 'ok'
          ? await this.turn({ tag: 'load', val: state }, undefined, undefined, false)
          : null
      const reply = loaded?.events[0]
      const matches =
        reply?.tag === 'ok' ? expected === null : isSuspension(reply) && samePending(expected, this.pending)
      if (!matches) {
        this.link.close()
        return null
      }
    } catch {
      this.link.close()
      return null
    }
    return this.turn(request, backstopMs, onPrint, false)
  }

  /** The turn's `DispatchResult`: its terminating event and the budget fields it reported. */
  private result({ event, frame }: Reply): DispatchResult {
    return {
      status: event.tag === 'fatal-error' || event.tag === 'shutdown' ? 'shutdown' : 'continue',
      events: [event],
      maxSuspensions: frame.maxSuspensions,
      maxTotalSleepMicros: frame.maxTotalSleepMicros,
      feedExecutionMicros: frame.feedExecutionMicros,
      maxFeedDurationMicros: frame.maxFeedDurationMicros,
      maxTurnDurationMicros: frame.maxTurnDurationMicros,
      ...(this.sessionId === undefined ? {} : { sessionId: this.sessionId }),
    }
  }
}

/** A turn's terminating event and the frame that carried it. */
interface Reply {
  event: Event
  frame: DecodedFrame
}

/** A suspension's identity, as `monty-pool`'s `PendingKey`. */
type PendingKey =
  | { kind: 'call'; callId: number; functionName: string }
  | { kind: 'lookup'; name: string; objectId: string | null }
  | { kind: 'futures'; callIds: number[] }

/** Whether a re-announced suspension is the one shut down. */
function samePending(a: PendingKey | null, b: PendingKey | null): boolean {
  if (a === null || b === null || a.kind !== b.kind) return false
  switch (a.kind) {
    case 'call':
      return b.kind === 'call' && a.callId === b.callId && a.functionName === b.functionName
    case 'lookup':
      return b.kind === 'lookup' && a.name === b.name && a.objectId === b.objectId
    case 'futures':
      return (
        b.kind === 'futures' && a.callIds.length === b.callIds.length && a.callIds.every((id, i) => id === b.callIds[i])
      )
  }
}

function isSuspension(event: Event | undefined): boolean {
  return (
    event?.tag === 'function-call' ||
    event?.tag === 'os-call' ||
    event?.tag === 'name-lookup' ||
    event?.tag === 'resolve-futures'
  )
}

/** The reply to a request that was never sent, as `monty-pool` answers an oversize one. */
function refused(message: string): DispatchResult {
  return {
    status: 'continue',
    events: [
      { tag: 'error', val: { excType: 'RuntimeError', message, traceback: `RuntimeError: ${message}`, frames: [] } },
    ],
    feedExecutionMicros: 0n,
  }
}

/**
 * One WebSocket: binary messages queue as frames until a turn reads them. A
 * text message or a close ends the link, as it does in `monty-pool`.
 */
class Link {
  private readonly frames: Uint8Array[] = []
  private closed = false
  private wake: (() => void) | null = null

  private constructor(private readonly socket: WebSocket) {
    socket.onmessage = (message: MessageEvent) => {
      if (message.data instanceof ArrayBuffer) {
        this.frames.push(new Uint8Array(message.data))
        this.notify()
      } else {
        this.close()
      }
    }
    socket.onerror = socket.onclose = () => this.close()
  }

  /** Dials `options.url`, bounded by the request timeout. */
  static open(options: RemoteOptions): Promise<Link> {
    return new Promise((resolve, reject) => {
      const fail = (reason: string) =>
        reject(new Error(`failed to connect to monty worker at ${options.url}: ${reason}`))
      let socket: WebSocket
      try {
        socket = new WebSocket(options.url)
      } catch (error) {
        fail(error instanceof Error ? error.message : String(error))
        return
      }
      socket.binaryType = 'arraybuffer'
      const timeoutMs = options.requestTimeoutMs
      const timer =
        timeoutMs === undefined
          ? null
          : deadlineTimer(timeoutMs, () => {
              socket.close()
              fail(`no answer within ${formatDuration(timeoutMs)}`)
            })
      socket.onopen = () => {
        timer?.cancel()
        resolve(new Link(socket))
      }
      // a browser reports no reason for a failed dial or upgrade, and some
      // runtimes follow `error` with `close` while others (Node's) do not
      socket.onerror = socket.onclose = () => {
        timer?.cancel()
        fail('the connection or upgrade was refused')
      }
    })
  }

  /** Sends `frame` and reads replies up to the turn-ending event, within `timeoutMs`. */
  async exchange(
    frame: Uint8Array,
    timeoutMs: number,
    decode: (frame: Uint8Array) => DecodedFrame,
    onPrint: OnPrint | undefined,
  ): Promise<Reply> {
    if (this.closed) throw disconnected('sending a request')
    this.socket.send(frame)
    const deadline = performance.now() + timeoutMs
    for (;;) {
      let decoded: DecodedFrame
      try {
        decoded = decode(await this.next(deadline, timeoutMs))
      } catch (error) {
        if (!(error instanceof CodecError)) throw error
        this.close()
        throw new DispatchFailure({ kind: 'protocol', message: error.message })
      }
      for (const event of decoded.events) {
        if (event.tag === 'print') onPrint?.(event.val.stderr ? 'stderr' : 'stdout', event.val.text)
        else return { event, frame: decoded }
      }
    }
  }

  /** Closes the socket; a turn waiting on it fails as disconnected. */
  close(): void {
    if (this.closed) return
    this.closed = true
    this.socket.close()
    this.notify()
  }

  /** The next frame, failing once the link closes or `deadline` passes. */
  private async next(deadline: number, timeoutMs: number): Promise<Uint8Array> {
    for (;;) {
      const frame = this.frames.shift()
      if (frame !== undefined) return frame
      if (this.closed) throw disconnected('waiting for a reply')
      const remaining = deadline - performance.now()
      if (remaining <= 0) {
        this.close()
        throw new DispatchFailure({
          kind: 'crashed',
          message: `monty worker killed after exceeding request timeout of ${formatDuration(timeoutMs)}`,
          timedOut: true,
        })
      }
      await new Promise<void>((resolve) => {
        const timer = Number.isFinite(remaining) ? deadlineTimer(remaining, resolve) : null
        this.wake = () => {
          timer?.cancel()
          resolve()
        }
      })
    }
  }

  private notify(): void {
    const wake = this.wake
    this.wake = null
    wake?.()
  }
}

/** The failure for a connection that closed mid-session, worded as `monty-pool`'s. */
function disconnected(context: string): DispatchFailure {
  return new DispatchFailure({ kind: 'disconnected', message: `monty worker connection closed while ${context}` })
}
