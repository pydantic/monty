// Locates the `monty` CLI binary that worker subprocesses run.
//
// Resolution order mirrors pydantic_monty's `_binary.py`:
// 1. an explicit `binaryPath` option,
// 2. the `MONTY_BIN` environment variable,
// 3. the platform-specific npm package (`@pydantic/monty-<platform>`,
//    installed automatically via optionalDependencies),
// 4. `monty` on PATH,
// 5. a cargo workspace `target/{debug,release}` build (development fallback).

import { existsSync } from 'node:fs'
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

/** The binary shipped by the platform-specific npm package, if installed. */
function platformPackageBinary(): string | null {
  const triple = platformTriple()
  if (triple === null) {
    return null
  }
  const require = createRequire(import.meta.url)
  try {
    return require.resolve(`@pydantic/monty-${triple}/${EXE}`)
  } catch {
    return null
  }
}

/** Scans PATH directories for the binary. */
function searchPath(): string | null {
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (dir === '') {
      continue
    }
    const candidate = join(dir, EXE)
    if (existsSync(candidate)) {
      return candidate
    }
  }
  return null
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
