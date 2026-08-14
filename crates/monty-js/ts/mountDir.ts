// The `MountDir` class: a mount that owns its opened host directory. Separate
// from `mount.ts` because constructing one loads the native addon, and the
// wasm bundle imports `mount.ts` (via `session.ts`) but has no addon.

import { NativeMountDir } from '../native-addon.js'
import {
  DEFAULT_MEMORY_USAGE_LIMIT,
  NATIVE_MOUNT,
  VALID_MODES,
  type MountDirMode,
  type MountDirOptions,
} from './mount.js'

/**
 * Mounts a real host directory into the sandbox at a virtual path.
 * Retained overlay data and filesystem results share a per-mount memory
 * budget, `memoryUsageLimit` (100 MB by default).
 *
 * Mounts passed to one feed must cover disjoint host directories: overlap
 * (the same directory, or one inside the other) is rejected when the feed
 * starts, since the stricter mount's mode could otherwise be bypassed
 * through the other mount's paths.
 *
 * Warning: with `mode: 'read-write'`, files written by sandboxed code persist
 * on the host and are untrusted; do not execute them. Importing counts as
 * executing, and the import can be indirect: Node resolves imports from
 * `node_modules` directories up the tree, so a writable mount inside a project
 * lets sandboxed code shadow a package that has not been loaded yet. Tools
 * also read files without an explicit import: `.git/hooks/*`, `Makefile`,
 * `.env`, CI config. If Python runs against the directory, the same applies to
 * `sys.path` imports; see the `MountDir` docs in the Python package.
 *
 * The `'overlay'` default keeps writes in memory, so nothing reaches the host
 * filesystem. Use `'read-write'` only with a directory that contains no code
 * or config.
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
   *  Symbol-keyed, so it stays off the public surface — see [`NATIVE_MOUNT`]. */
  readonly [NATIVE_MOUNT]: NativeMountDir

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
    this[NATIVE_MOUNT] = new NativeMountDir({
      virtualPath: this.virtualPath,
      hostPath: this.hostPath,
      mode: this.mode,
      memoryUsageLimit: this.memoryUsageLimit,
      ...(this.writeBytesLimit !== null ? { writeBytesLimit: this.writeBytesLimit } : {}),
    })
  }

  /** Releases the open host directory. Later feeds using this mount throw;
   *  the properties above keep answering. Idempotent.
   *
   *  Only Windows needs this: it refuses to rename or delete a directory while
   *  a handle to it is open, so a mount left open blocks the host from touching
   *  it, and JS has no deterministic drop. A feed already running is unaffected.
   */
  close(): void {
    this[NATIVE_MOUNT].close()
  }

  /** Closes the mount at the end of a `using` block. */
  [Symbol.dispose](): void {
    this.close()
  }

  /** Returns a string representation of the mount. */
  repr(): string {
    return `MountDir(host_path='${this.hostPath}', virtual_path='${this.virtualPath}', mode='${this.mode}')`
  }
}
