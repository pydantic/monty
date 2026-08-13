// Filesystem mounts: expose a host directory inside the sandbox at a virtual
// POSIX path. Mounts apply per-feed and are serviced entirely on the host
// side of the pool (the worker never sees host paths), so they work even for
// remote workers. OS calls the mounts do not cover bubble up to the `os`
// callback.
//
// The `MountDir` class lives in `mountDir.js` because constructing one loads
// the native addon; everything here stays importable from the wasm bundle,
// which shares `session.ts`.

import type { NativeMountDir } from '../native-addon.js'
import type { MountDir } from './mountDir.js'

// Mirrors monty-fs's DEFAULT_MEMORY_USAGE_LIMIT (100 MB in decimal bytes).
export const DEFAULT_MEMORY_USAGE_LIMIT = 100_000_000

/** Sandbox access mode for a mounted directory. */
export type MountDirMode = 'read-only' | 'read-write' | 'overlay'

/** Configuration for [`MountDir`]. The paths are named fields (never
 *  positional): mount tools disagree on host-first (docker `-v`) vs
 *  virtual-first (nginx `alias`) ordering, so requiring names removes the
 *  ambiguity. */
export interface MountDirOptions {
  /** Real host directory to expose. Sandbox code can never see this path or
   *  reach outside it. Opened by the constructor and held for the mount's
   *  lifetime, so on macOS/BSD it must be readable (a search-only directory
   *  cannot be mounted there) and, on Windows, cannot be renamed or deleted by
   *  anyone until the mount is dropped. The mount then follows that directory,
   *  not the path: renaming or replacing it afterwards changes nothing.
   *  Symlinks inside it are followed only if their targets are relative. */
  hostPath: string
  /** Absolute virtual POSIX path prefix inside the sandbox (e.g. `'/data'`),
   *  regardless of host OS. */
  virtualPath: string
  /**
   * Access mode (default `'overlay'`): `'read-only'` rejects writes,
   * `'read-write'` writes through to the host, `'overlay'` keeps writes in
   * memory and discards them when the feed ends.
   *
   * `'read-write'` leaves untrusted files on the host after the feed — see the
   * warning on [`MountDir`] before choosing it.
   */
  mode?: MountDirMode
  /** Cap on total bytes written through this mount. */
  writeBytesLimit?: number
  /** Aggregate mount memory budget in bytes (default 100 MB). */
  memoryUsageLimit?: number
}

export const VALID_MODES: Record<MountDirMode, true> = {
  'read-only': true,
  'read-write': true,
  overlay: true,
}

/** Key under which a `MountDir` carries its opened native handle.
 *
 *  `private` is what `Monty` and `MontySession` use, but it would put the
 *  handle out of reach of `mountsToNative` below, which lives in this module
 *  so the wasm bundle can import it without loading the addon. A symbol keeps
 *  the handle off the public surface — absent from `Object.keys`, JSON and
 *  completions — while both modules can still reach it. Deliberately not
 *  re-exported from `node.ts`. */
export const NATIVE_MOUNT: unique symbol = Symbol('nativeMount')

/** Collects the `mount` option (one or many) for the native binding. */
export function mountsToNative(mount: MountDir | MountDir[] | undefined): NativeMountDir[] {
  if (mount === undefined) {
    return []
  }
  const mounts = Array.isArray(mount) ? mount : [mount]
  return mounts.map((m) => m[NATIVE_MOUNT])
}
