// One `monty --subprocess` worker child: spawning, the Hello handshake, and
// framed request/event I/O. The crash-detection contract lives here: a child
// that exits or EOFs *without* sending `FatalError` crashed hard (segfault,
// allocator abort, kill) — exactly the failure mode subprocess isolation
// exists to contain.

import { spawn, type ChildProcess } from 'node:child_process'
import { create, fromBinary, toBinary } from '@bufbuild/protobuf'
import { MontyCrashedError } from './errors.js'
import { FrameReader, writeFrame } from './frame.js'
import {
  EventSchema,
  HelloSchema,
  RequestSchema,
  ShutdownSchema,
  type Event,
  type Request,
} from './generated/monty/v1/monty_pb.js'

/** The protocol version this client speaks (monty-proto's PROTOCOL_VERSION). */
export const PROTOCOL_VERSION = 1

/** Deadline for the Hello handshake of a freshly spawned worker. */
const HANDSHAKE_TIMEOUT_MS = 10_000

/**
 * Thrown when the child violates the wire protocol (frame desync, decode
 * failure, unexpected event, `FatalError`). The worker is unusable and gets
 * discarded; unlike [`MontyCrashedError`] this normally indicates a bug
 * rather than a sandbox crash.
 */
export class ProtocolError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'ProtocolError'
  }
}

/** A spawned worker process with framed protobuf I/O. */
export class Worker {
  private readonly proc: ChildProcess
  private readonly reader: FrameReader
  /** Set by the watchdog before killing, to classify the read failure. */
  killedForTimeout = false
  /** REPL sessions served, for `maxCheckoutsPerWorker` recycling. */
  checkoutsServed = 0
  /** Exit description (e.g. `exit code: 1`), populated when the child exits. */
  exitStatus: string | null = null

  private constructor(proc: ChildProcess) {
    this.proc = proc
    this.reader = new FrameReader(proc.stdout!)
    proc.on('exit', (code, signal) => {
      this.exitStatus = signal !== null ? `signal: ${signal}` : `exit code: ${code}`
    })
    // A worker dying while idle must not crash the host process; failures
    // surface as read/write errors on the next turn instead.
    proc.on('error', () => {})
    proc.stdin!.on('error', () => {})
  }

  /** Spawns a worker and completes the Hello handshake. */
  static async spawn(binaryPath: string, extraArgs: string[] = []): Promise<Worker> {
    // The worker runs untrusted code, so spawn it with an empty environment:
    // host secrets (API keys, tokens) must never be in the child's memory
    // where a sandbox escape or memory disclosure could reach them. The
    // worker reads no environment variables — sandbox `os.getenv` is an
    // OsCall answered by the host. Windows processes misbehave without
    // SystemRoot (CRT and WinAPI lookups); it names the OS install directory
    // and is not sensitive.
    const env: Record<string, string> = {}
    if (process.platform === 'win32' && process.env.SystemRoot !== undefined) {
      env.SystemRoot = process.env.SystemRoot
    }
    const proc = spawn(binaryPath, ['--subprocess', ...extraArgs], {
      stdio: ['pipe', 'pipe', 'inherit'],
      env,
    })
    await new Promise<void>((resolve, reject) => {
      proc.once('spawn', resolve)
      proc.once('error', (err) => reject(new Error(`failed to spawn monty worker: ${err.message}`)))
    })
    const worker = new Worker(proc)
    // The handshake involves no user code, so a fixed deadline is safe; it
    // turns a wedged/wrong binary into a clear error instead of hanging pool
    // creation forever (the per-turn requestTimeout only covers turns).
    const deadline = setTimeout(() => {
      worker.kill()
    }, HANDSHAKE_TIMEOUT_MS)
    try {
      await worker.send({
        case: 'hello',
        value: create(HelloSchema, { protocolVersion: PROTOCOL_VERSION, client: 'monty-js' }),
      })
      const reply = await worker.readEvent()
      if (reply.kind.case !== 'helloReply') {
        throw new ProtocolError(`expected HelloReply, got ${reply.kind.case ?? 'empty event'}`)
      }
    } catch (err) {
      worker.kill()
      throw err
    } finally {
      clearTimeout(deadline)
    }
    return worker
  }

  /** Sends one request frame. */
  async send(kind: Request['kind']): Promise<void> {
    const request = create(RequestSchema, { kind })
    try {
      await writeFrame(this.proc.stdin!, toBinary(RequestSchema, request))
    } catch (err) {
      throw await this.crashed(`failed to write to worker: ${(err as Error).message}`)
    }
  }

  /**
   * Reads the next event frame. EOF or a read error means the worker died —
   * the returned error is a [`MontyCrashedError`] classified via
   * `killedForTimeout` and the exit status.
   */
  async readEvent(): Promise<Event> {
    let frame: Buffer | null
    try {
      frame = await this.reader.read()
    } catch (err) {
      throw await this.crashed((err as Error).message)
    }
    if (frame === null) {
      throw await this.crashed('worker closed its output stream')
    }
    try {
      return fromBinary(EventSchema, frame)
    } catch (err) {
      throw new ProtocolError(`failed to decode event from worker: ${(err as Error).message}`)
    }
  }

  /** Builds the crash error for a dead worker, waiting briefly for its exit status. */
  private async crashed(context: string): Promise<MontyCrashedError> {
    const exitStatus = await this.waitExitStatus(200)
    if (this.killedForTimeout) {
      return new MontyCrashedError('the worker process was killed because a request timed out', {
        timedOut: true,
        exitStatus,
      })
    }
    const status = exitStatus === null ? '' : ` (${exitStatus})`
    return new MontyCrashedError(`the worker process crashed${status}: ${context}`, { exitStatus })
  }

  /** Waits up to `ms` for the exit status (the child may still be dying). */
  private waitExitStatus(ms: number): Promise<string | null> {
    if (this.exitStatus !== null || this.proc.exitCode !== null || this.proc.signalCode !== null) {
      return Promise.resolve(this.exitStatus)
    }
    return new Promise((resolve) => {
      const timer = setTimeout(() => resolve(this.exitStatus), ms)
      this.proc.once('exit', () => {
        clearTimeout(timer)
        resolve(this.exitStatus)
      })
    })
  }

  /** Whether the child process is still running. */
  get alive(): boolean {
    return this.proc.exitCode === null && this.proc.signalCode === null && this.exitStatus === null
  }

  /** OS process id, when the child is running. */
  get pid(): number | undefined {
    return this.proc.pid
  }

  /** Forcibly terminates the child. */
  kill(): void {
    this.proc.kill('SIGKILL')
  }

  /**
   * Asks the child to exit cleanly (`Shutdown` → `Ok` → exit 0), killing it
   * if it has not exited within the grace period.
   */
  async shutdown(graceMs = 1000): Promise<void> {
    if (!this.alive) {
      return
    }
    const exited = new Promise<void>((resolve) => {
      this.proc.once('exit', () => resolve())
    })
    try {
      await this.send({ case: 'shutdown', value: create(ShutdownSchema) })
      await Promise.race([exited, sleep(graceMs)])
    } catch {
      // Write failed — the child is already dead or dying.
    }
    if (this.alive) {
      this.kill()
      await exited
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
