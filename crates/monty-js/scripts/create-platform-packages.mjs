// Generates the per-platform npm packages that ship the `monty` CLI binary
// (the esbuild/ruff distribution pattern, mirroring what `napi
// create-npm-dirs` did for the old native binding): each package declares
// os/cpu/libc so npm installs only the matching one via the main package's
// optionalDependencies, and `src/binary.ts` resolves the binary from it.
//
// Usage:
//   node scripts/create-platform-packages.mjs           # all targets
//   node scripts/create-platform-packages.mjs --current # host target only
//
// CI copies the built binary into npm/<triple>/ before `npm publish`.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'))

const TARGETS = {
  'darwin-x64': { os: ['darwin'], cpu: ['x64'] },
  'darwin-arm64': { os: ['darwin'], cpu: ['arm64'] },
  'linux-x64-gnu': { os: ['linux'], cpu: ['x64'], libc: ['glibc'] },
  'linux-arm64-gnu': { os: ['linux'], cpu: ['arm64'], libc: ['glibc'] },
  'win32-x64-msvc': { os: ['win32'], cpu: ['x64'] },
}

function hostTriple() {
  const { platform, arch } = process
  if (platform === 'darwin') return `darwin-${arch}`
  if (platform === 'linux') return `linux-${arch}-gnu`
  if (platform === 'win32') return `win32-${arch}-msvc`
  throw new Error(`unsupported host platform: ${platform}-${arch}`)
}

const triples = process.argv.includes('--current') ? [hostTriple()] : Object.keys(TARGETS)

// The main package must depend on every platform package at the exact same
// version, otherwise npm installs a stale binary.
const expected = Object.fromEntries(Object.keys(TARGETS).map((t) => [`@pydantic/monty-${t}`, pkg.version]))
if (JSON.stringify(pkg.optionalDependencies ?? {}) !== JSON.stringify(expected)) {
  console.error('package.json optionalDependencies are out of sync with version/targets; expected:')
  console.error(JSON.stringify(expected, null, 2))
  process.exit(1)
}

for (const triple of triples) {
  const target = TARGETS[triple]
  if (target === undefined) {
    throw new Error(`unknown target triple: ${triple}`)
  }
  const dir = join(root, 'npm', triple)
  mkdirSync(dir, { recursive: true })
  const binary = triple.startsWith('win32') ? 'monty.exe' : 'monty'
  const platformPkg = {
    name: `@pydantic/monty-${triple}`,
    version: pkg.version,
    description: `The monty sandboxed Python interpreter binary for ${triple}, used by @pydantic/monty`,
    repository: pkg.repository,
    license: pkg.license,
    preferUnplugged: true,
    files: [binary],
    ...target,
    publishConfig: pkg.publishConfig,
  }
  writeFileSync(join(dir, 'package.json'), `${JSON.stringify(platformPkg, null, 2)}\n`)
  writeFileSync(
    join(dir, 'README.md'),
    `# @pydantic/monty-${triple}\n\nThe \`monty\` binary for ${triple}. Install [\`@pydantic/monty\`](https://www.npmjs.com/package/@pydantic/monty) instead of depending on this package directly.\n`,
  )
  console.log(`generated npm/${triple} (copy the ${binary} binary in before publishing)`)
}
