// Filesystem mounts: expose a host directory inside the sandbox at a virtual
// POSIX path. Mounts apply per-feed and are serviced entirely on the host
// side of the pool (the worker never sees host paths), so they work even for
// remote workers. OS calls the mounts do not cover bubble up to the `os`
// callback.

import type { NativeMount } from '../native-addon.js'

// Mirrors monty-fs's DEFAULT_MEMORY_USAGE_LIMIT (100 MB in decimal bytes).
const DEFAULT_MEMORY_USAGE_LIMIT = 100_000_000

/** Sandbox access mode for a mounted directory. */
export type MountDirMode = 'read-only' | 'read-write' | 'overlay'

/** Options for [`MountDir`]. */
export interface MountDirOptions {
  /**
   * Access mode (default `'overlay'`): `'read-only'` rejects writes,
   * `'read-write'` writes through to the host, `'overlay'` keeps writes in
   * memory and discards them when the feed ends.
   */
  mode?: MountDirMode
  /** Cap on total bytes written through this mount. */
  writeBytesLimit?: number
  /** Aggregate mount memory budget in bytes (default 100 MB). */
  memoryUsageLimit?: number
}

const VALID_MODES: Record<MountDirMode, true> = {
  'read-only': true,
  'read-write': true,
  overlay: true,
}

/**
 * Mounts a real host directory into the sandbox at a virtual path.
 * Retained overlay data and filesystem results share a per-mount memory
 * budget, `memoryUsageLimit` (100 MB by default).
 *
 * ```ts
 * const mount = new MountDir('/mnt/data', '/path/on/host', { mode: 'read-only' })
 * await session.feedRun("open('/mnt/data/file.txt').read()", { mount })
 * ```
 */
export class MountDir {
  readonly virtualPath: string
  readonly hostPath: string
  readonly mode: MountDirMode
  readonly writeBytesLimit: number | null
  readonly memoryUsageLimit: number

  constructor(virtualPath: string, hostPath: string, options: MountDirOptions = {}) {
    const mode = options.mode ?? 'overlay'
    // hasOwn, not `in`: prototype keys like 'toString' must not pass as modes
    if (!Object.hasOwn(VALID_MODES, mode)) {
      throw new Error(`invalid mount mode: '${mode}'. Expected 'read-only', 'read-write' or 'overlay'`)
    }
    this.virtualPath = virtualPath
    this.hostPath = hostPath
    this.mode = mode
    this.writeBytesLimit = options.writeBytesLimit ?? null
    this.memoryUsageLimit = options.memoryUsageLimit ?? DEFAULT_MEMORY_USAGE_LIMIT
    if (!Number.isSafeInteger(this.memoryUsageLimit) || this.memoryUsageLimit < 0) {
      throw new Error('memoryUsageLimit must be a non-negative safe integer')
    }
  }

  /** Returns a string representation of the mount. */
  repr(): string {
    return `MountDir(virtual_path='${this.virtualPath}', host_path='${this.hostPath}', mode='${this.mode}')`
  }
}

/** Encodes the `mount` option (one or many) for the native binding. */
export function mountsToNative(mount: MountDir | MountDir[] | undefined): NativeMount[] {
  if (mount === undefined) {
    return []
  }
  const mounts = Array.isArray(mount) ? mount : [mount]
  return mounts.map((m) => ({
    virtualPath: m.virtualPath,
    hostPath: m.hostPath,
    mode: m.mode,
    memoryUsageLimit: m.memoryUsageLimit,
    ...(m.writeBytesLimit !== null ? { writeBytesLimit: m.writeBytesLimit } : {}),
  }))
}
