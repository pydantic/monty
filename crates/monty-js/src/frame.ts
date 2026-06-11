// Wire framing for the subprocess protocol: each protobuf message is
// prefixed with a 4-byte unsigned little-endian length. Mirrors
// monty-proto's frame.rs, including the 256 MiB cap that stops a corrupted
// or byzantine child from making the parent allocate unbounded memory.

import type { Readable, Writable } from 'node:stream'

/** Maximum accepted frame payload (256 MiB), matching monty-proto. */
export const MAX_FRAME_LEN = 256 * 1024 * 1024

/**
 * Thrown on framing-level corruption (oversized length prefix, EOF mid-frame).
 * Unrecoverable for the connection — the worker must be discarded.
 */
export class FrameError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'FrameError'
  }
}

/**
 * Buffered frame reader over a worker's stdout.
 *
 * The protocol is strictly alternating, so a single pending `read()` at a
 * time is all we need: data events accumulate into a buffer and the pending
 * read resolves as soon as a complete frame is available. A clean EOF at a
 * frame boundary resolves to `null`; EOF mid-frame rejects with
 * [`FrameError`] (the worker died while writing).
 */
export class FrameReader {
  private chunks: Buffer[] = []
  private buffered = 0
  private ended = false
  private streamError: Error | null = null
  private pending: { resolve: (frame: Buffer | null) => void; reject: (err: Error) => void } | null = null

  constructor(stream: Readable) {
    stream.on('data', (chunk: Buffer) => {
      this.chunks.push(chunk)
      this.buffered += chunk.length
      this.poll()
    })
    stream.on('end', () => {
      this.ended = true
      this.poll()
    })
    stream.on('error', (err: Error) => {
      this.streamError = err
      this.poll()
    })
  }

  /** Reads the next frame payload, or `null` on clean EOF. */
  read(): Promise<Buffer | null> {
    if (this.pending) {
      return Promise.reject(new FrameError('concurrent frame reads are not supported'))
    }
    return new Promise((resolve, reject) => {
      this.pending = { resolve, reject }
      this.poll()
    })
  }

  /** Resolves the pending read if a full frame (or EOF/error) is available. */
  private poll(): void {
    const pending = this.pending
    if (!pending) {
      return
    }
    if (this.buffered >= 4) {
      const header = this.peek(4)
      const len = header.readUInt32LE(0)
      if (len > MAX_FRAME_LEN) {
        this.pending = null
        pending.reject(new FrameError(`frame of ${len} bytes exceeds maximum of ${MAX_FRAME_LEN}`))
        return
      }
      if (this.buffered >= 4 + len) {
        this.consume(4)
        const frame = this.take(len)
        this.pending = null
        pending.resolve(frame)
        return
      }
    }
    if (this.streamError) {
      this.pending = null
      pending.reject(new FrameError(`error reading from worker: ${this.streamError.message}`))
    } else if (this.ended) {
      this.pending = null
      if (this.buffered === 0) {
        pending.resolve(null)
      } else {
        pending.reject(new FrameError('worker closed mid-frame'))
      }
    }
  }

  /** Returns the first `n` buffered bytes without consuming them. */
  private peek(n: number): Buffer {
    if (this.chunks.length > 0 && this.chunks[0]!.length >= n) {
      return this.chunks[0]!
    }
    // Compact so the header is contiguous; rare in practice.
    const merged = Buffer.concat(this.chunks)
    this.chunks = [merged]
    return merged
  }

  /** Drops `n` buffered bytes. */
  private consume(n: number): void {
    this.take(n)
  }

  /** Removes and returns the first `n` buffered bytes. */
  private take(n: number): Buffer {
    const parts: Buffer[] = []
    let needed = n
    while (needed > 0) {
      const head = this.chunks[0]!
      if (head.length <= needed) {
        parts.push(head)
        needed -= head.length
        this.chunks.shift()
      } else {
        parts.push(head.subarray(0, needed))
        this.chunks[0] = head.subarray(needed)
        needed = 0
      }
    }
    this.buffered -= n
    return parts.length === 1 ? parts[0]! : Buffer.concat(parts)
  }
}

/**
 * Writes one framed message to the worker's stdin, waiting out backpressure
 * so the strict request/event alternation can never deadlock on a full pipe.
 */
export function writeFrame(stream: Writable, payload: Uint8Array): Promise<void> {
  const header = Buffer.allocUnsafe(4)
  header.writeUInt32LE(payload.length, 0)
  const frame = Buffer.concat([header, payload])
  return new Promise((resolve, reject) => {
    stream.write(frame, (err) => (err ? reject(err) : resolve()))
  })
}
