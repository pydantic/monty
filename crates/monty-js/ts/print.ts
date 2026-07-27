// Host-side print collectors for `printCallback` options.
//
// Pure TypeScript mirrors of pydantic_monty's CollectString / CollectStreams.
// Caps sit outside ResourceLimits.maxMemory (host process, not worker heap).
// CollectStreams charges a fixed per-entry overhead and does not merge fragments
// (Python pool host parity — not Rust in-process PrintWriter merge).

import { MontyRuntimeError } from './errors.js'

/** Default host collect cap (10 MiB). Mirrors monty::DEFAULT_MAX_PRINT_COLLECT_BYTES. */
export const DEFAULT_MAX_PRINT_COLLECT_BYTES = 10 * 1024 * 1024

/**
 * Host bytes charged per CollectStreams entry beyond payload.
 * Mirrors Python COLLECT_STREAMS_ENTRY_OVERHEAD (print_target.rs).
 * Package-internal (not re-exported); tests hardcode 64 / 265 in messages.
 */
const COLLECT_STREAMS_ENTRY_OVERHEAD = 64

const utf8Encoder = new TextEncoder()

/** UTF-8 byte length of `text` (parity with Rust/Python `str`/`String` len). */
function utf8Len(text: string): number {
  return utf8Encoder.encode(text).byteLength
}

/**
 * Reject collect growth past `maxBytes`.
 * `null` = unlimited. Message must match ResourceError::Memory / check_print_collect_limit exactly.
 * Package-internal — do not re-export from index/node/wasm.
 */
function checkPrintCollectLimit(current: number, add: number, maxBytes: number | null): void {
  if (maxBytes === null) return
  // Caps stay far below Number.MAX_SAFE_INTEGER; plain + is exact for our sizes.
  const used = current + add
  if (used > maxBytes) {
    throw new MontyRuntimeError('MemoryError', `memory limit exceeded: ${used} bytes > ${maxBytes} bytes`)
  }
}

/**
 * Normalize constructor `maxBytes`: only `null` disables the cap.
 * Rejects NaN/Infinity (which would make `used > maxBytes` never true) and negatives.
 */
function resolveMaxBytes(maxBytes: number | null): number | null {
  if (maxBytes === null) return null
  if (typeof maxBytes !== 'number' || !Number.isFinite(maxBytes) || maxBytes < 0) {
    throw new TypeError('maxBytes must be a finite non-negative number or null')
  }
  return maxBytes
}

/**
 * Accumulates print fragments into one string. Pass as `printCallback`.
 * Default cap: DEFAULT_MAX_PRINT_COLLECT_BYTES. Pass maxBytes: null to disable.
 * Host-side only — not covered by ResourceLimits.maxMemory.
 *
 * `maxBytes` must be a finite non-negative number or `null` (validated at construction).
 */
export class CollectString {
  private buf = ''
  private collectedBytes = 0
  private readonly maxBytes: number | null

  constructor(maxBytes: number | null = DEFAULT_MAX_PRINT_COLLECT_BYTES) {
    this.maxBytes = resolveMaxBytes(maxBytes)
  }

  /** Collected text so far (does not drain). Safe to read mid-feed or between feeds. */
  get output(): string {
    return this.buf
  }

  /**
   * Append one fragment. Invoked by PrintTarget; not part of the primary user API.
   * Throws MontyRuntimeError(MemoryError, ...) if this fragment would exceed the cap.
   * Check-before-append: only the *current* fragment is rejected — prior successful
   * writes remain in `output` / `collectedBytes` (same as Python). Never resets the buffer.
   */
  write(_stream: 'stdout' | 'stderr', text: string): void {
    const add = utf8Len(text)
    checkPrintCollectLimit(this.collectedBytes, add, this.maxBytes)
    this.buf += text
    this.collectedBytes += add
  }
}

/** One labelled print fragment retained by CollectStreams. */
export type CollectedStreamEntry = {
  stream: 'stdout' | 'stderr'
  text: string
}

/**
 * Accumulates labelled print fragments. Pass as `printCallback`.
 * Cap includes 64-byte per-entry overhead (Python host parity).
 * Does not merge consecutive same-stream fragments — pool events already fragment output.
 *
 * `maxBytes` must be a finite non-negative number or `null` (same contract as CollectString).
 */
export class CollectStreams {
  private entries: CollectedStreamEntry[] = []
  private collectedBytes = 0
  private readonly maxBytes: number | null

  constructor(maxBytes: number | null = DEFAULT_MAX_PRINT_COLLECT_BYTES) {
    this.maxBytes = resolveMaxBytes(maxBytes)
  }

  /** Collected fragments so far (cloned entries so callers cannot mutate internal state). */
  get output(): CollectedStreamEntry[] {
    return this.entries.map((e) => ({ stream: e.stream, text: e.text }))
  }

  /**
   * Append one labelled fragment. Check-before-push: only the current fragment is
   * rejected; prior successful entries remain (same as Python). Never clears the array.
   */
  write(stream: 'stdout' | 'stderr', text: string): void {
    const add = utf8Len(text) + COLLECT_STREAMS_ENTRY_OVERHEAD
    checkPrintCollectLimit(this.collectedBytes, add, this.maxBytes)
    this.entries.push({ stream, text })
    this.collectedBytes += add
  }
}
