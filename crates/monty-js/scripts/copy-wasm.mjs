// Copies the built lean wasm runtime module next to the packaged wasm loaders so
// `import.meta.url` resolution finds it. Run by `npm run build:wasm` after the
// cargo build.

import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const pkg = dirname(dirname(fileURLToPath(import.meta.url)))
const workspace = dirname(dirname(pkg))
const targetDir = process.env.CARGO_TARGET_DIR ?? join(workspace, 'target')
const src = join(targetDir, 'wasm32-wasip1', 'release', 'monty_wasm_runtime.wasm')

if (!existsSync(src)) {
  console.error(`missing ${src} — run 'cargo build -p monty-wasm-runtime --target wasm32-wasip1 --release' first`)
  process.exit(1)
}

const dest = join(pkg, 'dist', 'worker', 'monty_wasm_runtime.wasm')
mkdirSync(dirname(dest), { recursive: true })
copyFileSync(src, dest)
console.log(`copied wasm -> ${dest}`)
