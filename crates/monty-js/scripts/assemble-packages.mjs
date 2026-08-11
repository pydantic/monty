import { chmodSync, cpSync, existsSync, mkdirSync, readdirSync, renameSync, rmSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const artifacts = resolve(process.argv[2] ?? join(root, 'artifacts'))
const output = resolve(process.argv[3] ?? join(root, 'package-tarballs'))
const runtimeArtifacts = {
  'darwin-x64': 'pypi_files-macos-x86_64-manylinux-cli',
  'darwin-arm64': 'pypi_files-macos-pgo-cli',
  'linux-x64-gnu': 'pypi_files-linux-pgo-cli',
  'linux-arm64-gnu': 'pypi_files-linux-aarch64-manylinux-cli',
  'win32-x64-msvc': 'pypi_files-windows-pgo-cli',
}
const triples = Object.keys(runtimeArtifacts)

/** Finds exactly one downloaded artifact with the requested basename. */
function findArtifact(name, directory = artifacts) {
  const matches = []
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      matches.push(...findArtifact(name, path))
    } else if (entry.name === name) {
      matches.push(path)
    }
  }
  if (matches.length !== 1) throw new Error(`expected one ${name} in ${artifacts}, found ${matches.length}`)
  return matches[0]
}

/** Extracts the worker executable from a `pydantic-monty-runtime` wheel. */
function extractRuntime(triple, destination) {
  const wheelDirectory = join(artifacts, runtimeArtifacts[triple])
  // FIXME: maturin currently emits one identical runtime wheel per requested Python version.
  const wheels = readdirSync(wheelDirectory).filter((name) => /-cp310-.*\.whl$/.test(name))
  if (wheels.length !== 1)
    throw new Error(`expected one CPython 3.10 runtime wheel in ${wheelDirectory}, found ${wheels.length}`)
  const script = `
import sys, zipfile
wheel, binary, destination = sys.argv[1:]
with zipfile.ZipFile(wheel) as archive:
    matches = [name for name in archive.namelist() if '.data/scripts/' in name and name.endswith('/' + binary)]
    if len(matches) != 1:
        raise RuntimeError(f'expected one {binary} in {wheel}, found {len(matches)}')
    with archive.open(matches[0]) as source, open(destination, 'wb') as target:
        target.write(source.read())
`
  const binary = triple.startsWith('win32') ? 'monty.exe' : 'monty'
  execFileSync('python3', ['-c', script, join(wheelDirectory, wheels[0]), binary, destination])
}

/** Packs a package and verifies its published file set. */
function packAndValidate(directory, archiveName, requiredFiles) {
  const result = JSON.parse(
    execFileSync('npm', ['pack', '--json', '--pack-destination', output], { cwd: directory, encoding: 'utf8' }),
  )[0]
  const files = new Set(result.files.map(({ path }) => path))
  const missing = requiredFiles.filter((path) => !files.has(path))
  if (missing.length > 0) throw new Error(`${result.filename} is missing: ${missing.join(', ')}`)
  renameSync(join(output, result.filename), join(output, archiveName))
  console.log(`packed ${archiveName} (${result.files.length} files)`)
}

if (!existsSync(artifacts)) throw new Error(`artifact directory does not exist: ${artifacts}`)
rmSync(join(root, 'npm'), { recursive: true, force: true })
rmSync(output, { recursive: true, force: true })
mkdirSync(output, { recursive: true })

execFileSync('npx', ['napi', 'create-npm-dirs'], { cwd: root, stdio: 'inherit' })
execFileSync('node', ['scripts/create-platform-packages.mjs'], { cwd: root, stdio: 'inherit' })

for (const triple of triples) {
  const binary = triple.startsWith('win32') ? 'monty.exe' : 'monty'
  const platformDirectory = join(root, 'npm', triple)
  const addonArtifacts = join(artifacts, `monty-addon-${triple}`)
  cpSync(findArtifact(`monty.${triple}.node`, addonArtifacts), join(platformDirectory, `monty.${triple}.node`))
  const installedBinary = join(platformDirectory, binary)
  extractRuntime(triple, installedBinary)
  if (!triple.startsWith('win32')) chmodSync(installedBinary, 0o755)
  packAndValidate(platformDirectory, `monty-${triple}.tgz`, ['package.json', `monty.${triple}.node`, binary])
}

const wasm = join(root, 'dist', 'worker', 'monty_wasm_runtime.wasm')
if (!existsSync(wasm)) throw new Error(`missing wasm runtime: ${wasm}`)
packAndValidate(root, 'monty-main.tgz', [
  'dist/index.js',
  'dist/node.js',
  'dist/worker/index.js',
  'dist/worker/index.node.js',
  'dist/worker/index.browser.js',
  'dist/worker/monty_wasm_runtime.wasm',
  'native-addon.js',
  'native-addon.d.ts',
])
