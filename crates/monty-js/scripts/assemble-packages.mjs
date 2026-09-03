import { chmodSync, cpSync, existsSync, mkdirSync, readdirSync, renameSync, rmSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const artifacts = resolve(process.argv[2] ?? join(root, 'artifacts'))
const output = resolve(process.argv[3] ?? join(root, 'package-tarballs'))
const runtimeArtifacts = {
  'darwin-x64': 'pypi_files-runtime-macos-x86_64-manylinux',
  'darwin-arm64': 'pypi_files-runtime-macos-aarch64-manylinux',
  'linux-x64-gnu': 'pypi_files-runtime-linux-x86_64-manylinux',
  'linux-arm64-gnu': 'pypi_files-runtime-linux-aarch64-manylinux',
  'win32-x64-msvc': 'pypi_files-runtime-windows-x86_64-manylinux',
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
  const wheels = readdirSync(wheelDirectory).filter((name) => /-py3-none-.*\.whl$/.test(name))
  if (wheels.length !== 1)
    throw new Error(`expected one Python-independent runtime wheel in ${wheelDirectory}, found ${wheels.length}`)
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

const component = join(root, 'dist', 'worker', 'component', 'monty.component.js')
if (!existsSync(component)) throw new Error(`missing wasm component bindings: ${component}`)
packAndValidate(root, 'monty-main.tgz', [
  'dist/index.js',
  'dist/node.js',
  'dist/worker/index.js',
  'dist/worker/index.node.js',
  'dist/worker/index.browser.js',
  'dist/worker/component/monty.component.js',
  'dist/worker/component/monty.component.core.wasm',
  'dist/worker/component/monty.component.core2.wasm',
  'dist/worker/component/monty.component.core3.wasm',
  'dist/worker/component/monty.component.core4.wasm',
  'native-addon.js',
  'native-addon.d.ts',
])
