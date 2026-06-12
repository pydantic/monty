// Filesystem mounts: expose a host directory inside the sandbox at a virtual
// POSIX path. Mounts are sent per-feed and handled entirely inside the
// worker, so the host path must be valid on the machine the worker runs on.
// OS calls the mounts do not cover bubble up to the `os` callback.

import type { NativeMount } from '../index.js'

/** Sandbox access mode for a mounted directory. */
export type MountDirMode = 'read-only' | 'read-write' | 'overlay'

/** Options for [`MountDir`]. */
export interface MountDirOptions {
  /**
   * Access mode (default `'overlay'`): `'read-only'` rejects writes,
   * `'read-write'` writes through to the host, `'overlay'` keeps writes in
   * worker-local memory and discards them when the feed ends.
   */
  mode?: MountDirMode
  /** Cap on total bytes written through this mount. */
  writeBytesLimit?: number
}

const VALID_MODES: Record<MountDirMode, true> = {
  'read-only': true,
  'read-write': true,
  overlay: true,
}

/**
 * Mounts a real host directory into the sandbox at a virtual path.
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
    ...(m.writeBytesLimit !== null ? { writeBytesLimit: m.writeBytesLimit } : {}),
  }))
}
