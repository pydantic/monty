// Drives a Monty worker over a message channel (a Web Worker, or Node's
// `worker_threads`) as a [`PooledWorker`].
//
// This is where the browser model earns its keep over the in-process one: each
// turn is a `postMessage` round-trip, and a turn that runs too long is stopped
// by `terminate()` — the hard, unconditional kill the in-process path lacks.
// The channel correlates replies to requests by id, arms a per-turn watchdog,
// and rejects every in-flight request when the worker dies or is killed (so the
// transport sees a crash and the pool replaces it).

import type { DecodedChildEvent, Dispatcher } from './host.js'
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
 * The slice of a worker handle the channel needs, satisfied structurally by a
 * browser `Worker` (via a tiny adapter for its event API) and a Node
 * `worker_threads.Worker`.
 */
export interface WorkerLike {
  post(message: DispatchRequest): void
  onMessage(handler: (reply: DispatchReply) => void): void
  onError(handler: (err: unknown) => void): void
  terminate(): void
}

export interface WorkerChannelOptions {
  /** Hard per-turn deadline; on expiry the worker is terminated. */
  requestTimeoutMs?: number
}

interface Pending {
  resolve(value: { reply: Uint8Array; status: number; events?: DecodedChildEvent[] }): void
  reject(err: Error): void
  timer: ReturnType<typeof setTimeout> | null
}

/** A pooled worker backed by a message channel. */
export class WorkerChannel implements PooledWorker {
  private nextId = 1
  private readonly pending = new Map<number, Pending>()
  private live = true

  constructor(
    private readonly worker: WorkerLike,
    private readonly options: WorkerChannelOptions = {},
  ) {
    worker.onMessage((reply) => this.onReply(reply))
    worker.onError((err) => this.kill(new Error(`worker error: ${String(err)}`)))
  }

  get alive(): boolean {
    return this.live
  }

  /** Posts one turn and resolves with its reply, or rejects on death/timeout. */
  dispatch: Dispatcher = (frame) => {
    if (!this.live) return Promise.reject(new Error('worker is dead'))
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      const timeoutMs = this.options.requestTimeoutMs
      const timer = timeoutMs === undefined ? null : setTimeout(() => this.onTimeout(), timeoutMs)
      this.pending.set(id, { resolve, reject, timer })
      this.worker.post({ id, frame })
    })
  }

  /** Hard-kills the worker; any in-flight turn rejects. */
  terminate(): void {
    this.kill(new Error('worker terminated'))
  }

  private onReply(reply: DispatchReply): void {
    const pending = this.pending.get(reply.id)
    if (!pending) return
    this.pending.delete(reply.id)
    if (pending.timer) clearTimeout(pending.timer)
    pending.resolve({ reply: reply.reply, status: reply.status })
  }

  private onTimeout(): void {
    // a synchronous sandbox turn can only be stopped by killing the worker
    this.kill(new Error('turn exceeded the request timeout'))
  }

  private kill(err: Error): void {
    if (!this.live) return
    this.live = false
    this.worker.terminate()
    for (const pending of this.pending.values()) {
      if (pending.timer) clearTimeout(pending.timer)
      pending.reject(err)
    }
    this.pending.clear()
  }
}
