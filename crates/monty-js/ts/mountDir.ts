// The `MountDir` class: a mount that owns its opened host directory. Separate
// from `mount.ts` because constructing one loads the native addon, and the
// wasm bundle imports `mount.ts` (via `session.ts`) but has no addon.

import { NativeMountDir } from '../native-addon.js'
import { DEFAULT_MEMORY_USAGE_LIMIT, VALID_MODES, type MountDirMode, type MountDirOptions } from './mount.js'

/**
 * Mounts a real host directory into the sandbox at a virtual path.
 * Retained overlay data and filesystem results share a per-mount memory
 * budget, `memoryUsageLimit` (100 MB by default).
 *
 * ```ts
 * const mount = new MountDir({ hostPath: '/path/on/host', virtualPath: '/mnt/data', mode: 'read-only' })
 * await session.feedRun("open('/mnt/data/file.txt').read()", { mount })
 * ```
 */
export class MountDir {
  readonly hostPath: string
  readonly virtualPath: string
  readonly mode: MountDirMode
  readonly writeBytesLimit: number | null
  readonly memoryUsageLimit: number
  /** The opened host directory, shared by every feed this mount is passed to.
   *  @internal */
  readonly native: NativeMountDir

  constructor(options: MountDirOptions) {
    const mode = options.mode ?? 'overlay'
    // hasOwn, not `in`: prototype keys like 'toString' must not pass as modes
    if (!Object.hasOwn(VALID_MODES, mode)) {
      throw new Error(`invalid mount mode: '${mode}'. Expected 'read-only', 'read-write' or 'overlay'`)
    }
    this.hostPath = options.hostPath
    this.virtualPath = options.virtualPath
    this.mode = mode
    this.writeBytesLimit = options.writeBytesLimit ?? null
    this.memoryUsageLimit = options.memoryUsageLimit ?? DEFAULT_MEMORY_USAGE_LIMIT
    if (!Number.isSafeInteger(this.memoryUsageLimit) || this.memoryUsageLimit < 0) {
      throw new Error('memoryUsageLimit must be a non-negative safe integer')
    }
    // Opens the directory, so a bad host path throws here rather than at the
    // first feed — and every feed then mounts this descriptor, which no rename
    // of the host path can redirect.
    this.native = new NativeMountDir({
      virtualPath: this.virtualPath,
      hostPath: this.hostPath,
      mode: this.mode,
      memoryUsageLimit: this.memoryUsageLimit,
      ...(this.writeBytesLimit !== null ? { writeBytesLimit: this.writeBytesLimit } : {}),
    })
  }

  /** Returns a string representation of the mount. */
  repr(): string {
    return `MountDir(host_path='${this.hostPath}', virtual_path='${this.virtualPath}', mode='${this.mode}')`
  }
}
