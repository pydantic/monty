// Locates the `monty` CLI binary that worker subprocesses run.
//
// Resolution order mirrors pydantic_monty's `_binary.py`:
// 1. an explicit `binaryPath` option,
// 2. the `MONTY_BIN` environment variable,
// 3. the platform-specific npm package (`@pydantic/monty-<platform>`,
//    installed automatically via optionalDependencies),
// 4. `monty` on PATH,
// 5. a cargo workspace `target/{debug,release}` build (development fallback).

import { accessSync, constants, existsSync, statSync } from 'node:fs'
import { createRequire } from 'node:module'
import { delimiter, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const EXE = process.platform === 'win32' ? 'monty.exe' : 'monty'

/**
 * The napi-style platform triple used to name binary packages, or `null` on
 * platforms we do not ship binaries for.
 */
export function platformTriple(): string | null {
  const { platform, arch } = process
  if (platform === 'darwin' && (arch === 'x64' || arch === 'arm64')) {
    return `darwin-${arch}`
  }
  if (platform === 'linux' && (arch === 'x64' || arch === 'arm64')) {
    return `linux-${arch}-gnu`
  }
  if (platform === 'win32' && arch === 'x64') {
    return 'win32-x64-msvc'
  }
  return null
}

/**
 * Resolves the `monty` binary path, throwing a descriptive error naming
 * every location tried when nothing is found.
 */
export function findMontyBinary(explicit?: string): string {
  if (explicit !== undefined) {
    if (!existsSync(explicit)) {
      throw new Error(`monty binary not found at binaryPath: ${explicit}`)
    }
    return explicit
  }

  const tried: string[] = []

  const envBin = process.env.MONTY_BIN
  if (envBin) {
    if (existsSync(envBin)) {
      return envBin
    }
    tried.push(`MONTY_BIN=${envBin}`)
  }

  const fromPackage = platformPackageBinary()
  if (fromPackage !== null) {
    return fromPackage
  }
  tried.push('platform package @pydantic/monty-<platform>')

  const fromPath = searchPath()
  if (fromPath !== null) {
    return fromPath
  }
  tried.push('PATH')

  const fromWorkspace = workspaceBinary()
  if (fromWorkspace !== null) {
    return fromWorkspace
  }
  tried.push('cargo workspace target/')

  throw new Error(
    `could not locate the monty binary (tried: ${tried.join(', ')}). ` +
      'Install the platform package, set MONTY_BIN, or pass binaryPath.',
  )
}

/**
 * The binary shipped by the platform-specific npm package, if installed.
 *
 * Resolution failures fall through to the next strategy rather than erroring:
 * the same package names previously shipped napi `.node` bindings, so a stale
 * install can resolve while holding no `monty` executable.
 */
function platformPackageBinary(): string | null {
  const triple = platformTriple()
  if (triple === null) {
    return null
  }
  const require = createRequire(import.meta.url)
  try {
    return require.resolve(`@pydantic/monty-${triple}/${EXE}`)
  } catch (err) {
    // Only not-found-style failures fall through (package not installed, or
    // installed without the binary). Anything else — permission errors,
    // malformed package.json — is a real problem the user must see, not a
    // cue to silently pick a different binary.
    const code = (err as NodeJS.ErrnoException).code
    if (code === 'MODULE_NOT_FOUND' || code === 'ERR_MODULE_NOT_FOUND' || code === 'ERR_PACKAGE_PATH_NOT_EXPORTED') {
      return null
    }
    throw err
  }
}

/** Scans PATH directories for an executable `monty`. */
function searchPath(): string | null {
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (dir === '') {
      continue
    }
    const candidate = join(dir, EXE)
    if (isExecutableFile(candidate)) {
      return candidate
    }
  }
  return null
}

/** Whether `path` is a regular file the current process may execute. */
function isExecutableFile(path: string): boolean {
  try {
    if (!statSync(path).isFile()) {
      return false
    }
    // X_OK is meaningless on Windows, where any readable file is "executable"
    accessSync(path, process.platform === 'win32' ? constants.R_OK : constants.X_OK)
    return true
  } catch {
    return false
  }
}

/**
 * Development fallback: walk up from this file looking for a cargo workspace
 * containing a built `monty` binary (debug preferred — it matches the code
 * being developed; release as fallback).
 */
function workspaceBinary(): string | null {
  let dir = dirname(fileURLToPath(import.meta.url))
  for (let i = 0; i < 6; i++) {
    if (existsSync(join(dir, 'Cargo.toml'))) {
      for (const profile of ['debug', 'release']) {
        const candidate = join(dir, 'target', profile, EXE)
        if (existsSync(candidate)) {
          return candidate
        }
      }
    }
    const parent = resolve(dir, '..')
    if (parent === dir) {
      break
    }
    dir = parent
  }
  return null
}
