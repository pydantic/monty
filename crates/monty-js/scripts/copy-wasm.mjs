// Copies the built lean worker module next to the wasm loaders so
// `import.meta.url` resolution finds it. Run by `npm run build:wasm` after the
// cargo build.
//
// Two destinations: `ts/worker/` (for dev/test, where ava runs the .ts sources
// via @oxc-node/core) and `dist/worker/` (the published package, where `files`
// ships `dist`). Both are gitignored generated artifacts.

import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const pkg = dirname(dirname(fileURLToPath(import.meta.url)))
const workspace = dirname(dirname(pkg))
const src = join(workspace, 'target', 'wasm32-wasip1', 'release', 'monty_wasm.wasm')

if (!existsSync(src)) {
  console.error(`missing ${src} — run 'cargo build -p monty-wasm --target wasm32-wasip1 --release' first`)
  process.exit(1)
}

for (const dest of [join(pkg, 'ts', 'worker', 'monty_wasm.wasm'), join(pkg, 'dist', 'worker', 'monty_wasm.wasm')]) {
  mkdirSync(dirname(dest), { recursive: true })
  copyFileSync(src, dest)
  console.log(`copied wasm -> ${dest}`)
}
