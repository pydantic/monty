// Drives a Monty worker over a message channel (a Web Worker, or Node's
// `worker_threads`) as a [`PooledWorker`].
//
// Each turn is a `postMessage` round-trip. The channel correlates replies,
// applies the request and duration-backstop deadlines, and preserves the
// runtime's timeout/exit metadata when a worker dies.

import { deadlineTimer, type DeadlineTimer } from './deadline.js'
import { DispatchError, type DecodedChildEvent, type DispatchResult, type Dispatcher } from './host.js'
import type { PooledWorker } from './pool.js'

/** A request sent to the worker: a turn's framed `ParentRequest`. */
export interface DispatchRequest {
  id: number
  frame: Uint8Array
}

/** A reply from the worker: a turn's framed `ChildEvent`s. */
export interface DispatchReply {
  id: number
  reply: Uint8Array
  status: number
  events?: DecodedChildEvent[]
}

/**
 * The worker operations shared by browser `Worker` and Node
 * `worker_threads.Worker` adapters.
 */
export interface WorkerLike {
  post(message: DispatchRequest): void
  onMessage(handler: (reply: DispatchReply) => void): void
  onError(handler: (err: unknown) => void): void
  /** Registers process/thread exit notification when the runtime provides it. */
  onExit?(handler: (exitStatus: string | null) => void): void
  /** Stops the worker, resolving to an exit description when available. */
  terminate(): void | string | null | Promise<string | null>
}

export interface WorkerChannelOptions {
  /** Hard per-turn deadline applied independently of duration backstops. */
  requestTimeoutMs?: number
}

interface Pending {
  resolve(value: DispatchResult): void
  reject(err: Error): void
  timer: DeadlineTimer | null
}

/** A pooled worker backed by a message channel. */
export class WorkerChannel implements PooledWorker {
  private nextId = 1
  private readonly pending = new Map<number, Pending>()
  private live = true
  private stopping: Promise<string | null> | null = null
  private knownExitStatus: string | null = null

  constructor(
    private readonly worker: WorkerLike,
    private readonly options: WorkerChannelOptions = {},
    readonly workerId?: number,
  ) {
    worker.onMessage((reply) => this.onReply(reply))
    worker.onError((err) => void this.kill(`worker error: ${errorMessage(err)}`))
    worker.onExit?.((exitStatus) => this.onExit(exitStatus))
  }

  get alive(): boolean {
    return this.live
  }

  /** Posts one turn and resolves with its reply, or rejects on death/timeout. */
  dispatch: Dispatcher = (frame, options = {}) => {
    if (!this.live) return Promise.reject(new DispatchError('worker is dead', false, this.knownExitStatus))
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      const timeoutMs = minimumTimeout(this.options.requestTimeoutMs, options.timeoutMs)
      const timer = timeoutMs === undefined ? null : deadlineTimer(timeoutMs, () => void this.onTimeout(timeoutMs))
      this.pending.set(id, { resolve, reject, timer })
      try {
        this.worker.post({ id, frame })
      } catch (err) {
        void this.kill(`worker channel failed: ${errorMessage(err)}`)
      }
    })
  }

  /** Hard-kills and reaps the worker; any in-flight turn rejects. */
  terminate(): Promise<void> {
    return this.kill('worker terminated')
  }

  private onReply(reply: DispatchReply): void {
    const pending = this.pending.get(reply.id)
    if (!pending) return
    this.pending.delete(reply.id)
    pending.timer?.cancel()
    const result = { reply: reply.reply, status: reply.status, events: reply.events }
    if (reply.status === 0) {
      pending.resolve(result)
    } else {
      // A FatalError/IO failure is the worker's final reply. Reap the Node
      // thread before resolving so the transport can attach its exit code.
      this.live = false
      void this.stopWorker().then((exitStatus) => {
        pending.resolve({ ...result, exitStatus })
        this.rejectPending(new DispatchError('worker terminated', false, exitStatus))
      })
    }
  }

  private onTimeout(timeoutMs: number): void {
    void this.kill(`monty worker killed after exceeding request timeout of ${formatDuration(timeoutMs)}`, true)
  }

  private onExit(exitStatus: string | null): void {
    this.knownExitStatus = exitStatus
    if (this.live) {
      this.live = false
      this.rejectPending(new DispatchError('worker exited', false, exitStatus))
    }
  }

  private async kill(message: string, timedOut = false): Promise<void> {
    if (!this.live) {
      await this.stopping
      return
    }
    this.live = false
    const pending = [...this.pending.values()]
    this.pending.clear()
    for (const item of pending) item.timer?.cancel()
    const exitStatus = await this.stopWorker()
    const error = new DispatchError(message, timedOut, exitStatus)
    for (const item of pending) item.reject(error)
  }

  private stopWorker(): Promise<string | null> {
    this.stopping ??= Promise.resolve(this.worker.terminate())
      .then((exitStatus) => exitStatus ?? this.knownExitStatus)
      .catch(() => this.knownExitStatus)
    return this.stopping
  }

  private rejectPending(error: DispatchError): void {
    for (const pending of this.pending.values()) {
      pending.timer?.cancel()
      pending.reject(error)
    }
    this.pending.clear()
  }
}

/** Chooses the earlier configured deadline, as the native pool does. */
function minimumTimeout(
  requestTimeoutMs: number | undefined,
  backstopTimeoutMs: number | undefined,
): number | undefined {
  if (requestTimeoutMs === undefined) return backstopTimeoutMs
  if (backstopTimeoutMs === undefined) return requestTimeoutMs
  return Math.min(requestTimeoutMs, backstopTimeoutMs)
}

/** Formats milliseconds like Rust's `Duration` debug output used natively. */
function formatDuration(ms: number): string {
  if (ms >= 1000) return `${ms / 1000}s`
  if (ms >= 1) return `${ms}ms`
  if (ms >= 0.001) return `${ms * 1000}µs`
  return `${ms * 1_000_000}ns`
}

/** Extracts a useful message from an arbitrary worker error event. */
function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}
