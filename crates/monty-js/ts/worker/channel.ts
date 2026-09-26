// Shared request correlation, hard deadlines and termination for browser and Node workers.

import { MontyCrashedError } from '../errors.js'
import { deadlineTimer, type DeadlineTimer } from './deadline.js'
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

/** Initialization is acknowledged before any turn deadline starts. */
export type WorkerMessage = DispatchReply | { ready: true } | { startupError: string }

/** The message and lifecycle operations supplied by each worker backend. */
export interface WorkerLike {
  post(message: DispatchRequest): void
  onMessage(handler: (reply: WorkerMessage) => void): void
  onError(handler: (err: unknown) => void): void
  onExit?(handler: (exitStatus: string | null) => void): void
  terminate(): void | Promise<string | null>
}

/** Hard per-turn deadline independent of the interpreter's duration limits. */
export interface WorkerChannelOptions {
  requestTimeoutMs?: number
}

/** One request awaiting either its reply or worker termination. */
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
  private failure: MontyCrashedError | null = null
  private readonly ready: Promise<void>
  private markReady!: () => void
  private failStartup!: (error: Error) => void

  /** Waits for component initialization, independently of per-request deadlines. */
  static async create(
    worker: WorkerLike,
    options: WorkerChannelOptions = {},
    signal?: AbortSignal,
  ): Promise<WorkerChannel> {
    const channel = new WorkerChannel(worker, options)
    const cancel = () => void channel.kill('worker initialization cancelled')
    signal?.addEventListener('abort', cancel, { once: true })
    if (signal?.aborted) cancel()
    const timer = deadlineTimer(30_000, () => void channel.kill('monty worker initialization timed out', true))
    try {
      await channel.ready
      return channel
    } finally {
      timer.cancel()
      signal?.removeEventListener('abort', cancel)
    }
  }

  private constructor(
    private readonly worker: WorkerLike,
    private readonly options: WorkerChannelOptions = {},
  ) {
    this.ready = new Promise((resolve, reject) => {
      this.markReady = resolve
      this.failStartup = reject
    })
    worker.onMessage((reply) => {
      if ('ready' in reply) this.markReady()
      else if ('startupError' in reply) void this.kill(`worker initialization failed: ${reply.startupError}`)
      else this.onReply(reply)
    })
    worker.onError(() => void this.kill('worker exited without a turn-ending event'))
    worker.onExit?.((exitStatus) => {
      if (this.live) void this.kill('worker exited without a turn-ending event', false, exitStatus)
    })
  }

  get alive(): boolean {
    return this.live
  }

  /** Posts one turn and resolves with its reply, or rejects on death/timeout. */
  dispatch: Dispatcher = (request, backstopMs) => {
    if (!this.live) return Promise.reject(this.failure ?? new MontyCrashedError('worker is dead'))
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      const timeoutMs = Math.min(this.options.requestTimeoutMs ?? Infinity, backstopMs ?? Infinity)
      const timer = Number.isFinite(timeoutMs)
        ? deadlineTimer(
            timeoutMs,
            () =>
              void this.kill(
                `monty worker killed after exceeding request timeout of ${formatDuration(timeoutMs)}`,
                true,
              ),
          )
        : null
      this.pending.set(id, { resolve, reject, timer })
      try {
        this.worker.post({ id, request })
      } catch {
        void this.kill('worker exited without a turn-ending event')
      }
    })
  }

  /** Hard-kills and reaps the worker; in-flight turns reject. */
  async terminate(): Promise<void> {
    await this.kill('worker terminated')
  }

  /** Completes exactly one turn, retiring a component that reported shutdown. */
  private onReply(reply: DispatchReply): void {
    const pending = this.pending.get(reply.id)
    if (!pending) return
    this.pending.delete(reply.id)
    pending.timer?.cancel()
    if (reply.status === 'shutdown') {
      void this.kill('worker exited without a turn-ending event').then(() =>
        pending.resolve({ ...reply, exitStatus: this.failure?.exitStatus }),
      )
    } else {
      pending.resolve(reply)
    }
  }

  /** Reaps the worker before exposing its final crash metadata to callers. */
  private async kill(message: string, timedOut = false, knownExitStatus: string | null = null): Promise<void> {
    if (!this.live) {
      await this.stopping
      return
    }
    this.live = false
    const pending = [...this.pending.values()]
    this.pending.clear()
    for (const item of pending) item.timer?.cancel()
    this.failure = new MontyCrashedError(message, { timedOut, exitStatus: knownExitStatus })
    this.stopping = Promise.resolve(this.worker.terminate()).then(
      (status) => knownExitStatus ?? status ?? null,
      () => knownExitStatus,
    )
    const exitStatus = await this.stopping
    this.failure = new MontyCrashedError(message, { timedOut, exitStatus })
    this.failStartup(this.failure)
    for (const item of pending) item.reject(this.failure)
  }
}

/** Formats milliseconds like the native pool's duration diagnostics. */
function formatDuration(ms: number): string {
  if (ms >= 1000) return `${ms / 1000}s`
  if (ms >= 1) return `${ms}ms`
  if (ms >= 0.001) return `${ms * 1000}µs`
  return `${ms * 1_000_000}ns`
}
