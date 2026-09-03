// Drives a Monty worker over a message channel (a Web Worker, or Node's
// `worker_threads`) as a [`PooledWorker`].
//
// Each turn is a `postMessage` round-trip. The channel correlates replies to
// requests by id, arms a per-turn watchdog, and rejects every in-flight request
// when the worker dies or is killed.

import type { DispatchRequest as ComponentRequest, DispatchResult, Dispatcher } from './host.js'
import type { PooledWorker } from './pool.js'

/** A semantic component request sent to a worker. */
export interface DispatchRequest {
  id: number
  request: ComponentRequest
}

/** A semantic component reply sent back by a worker. */
export interface DispatchReply extends DispatchResult {
  id: number
}

/**
 * The slice of a worker handle the channel needs, satisfied structurally by a
 * browser `Worker` and a Node `worker_threads.Worker`.
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
  resolve(value: DispatchResult): void
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
  dispatch: Dispatcher = (request) => {
    if (!this.live) return Promise.reject(new Error('worker is dead'))
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      const timeoutMs = this.options.requestTimeoutMs
      const timer = timeoutMs === undefined ? null : setTimeout(() => this.onTimeout(), timeoutMs)
      this.pending.set(id, { resolve, reject, timer })
      this.worker.post({ id, request })
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
    pending.resolve({ status: reply.status, events: reply.events, maxSuspensions: reply.maxSuspensions })
  }

  private onTimeout(): void {
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
