import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { test } from 'vitest'

import { t } from './assertions.js'

// `@pydantic/monty`, `@pydantic/monty/node` and `@pydantic/monty/wasm` are
// the same API over two transports, so a type reaching only one of them is a
// packaging bug — the entrypoints re-export by hand and have drifted before.
const ENTRYPOINTS = ['../ts/index.ts', '../ts/node.ts', '../ts/worker/index.ts']

/** The sorted names in `path`'s `export { ... } from '<module>'` block. */
function exportsOf(path: string, module: string): string[] {
  const source = readFileSync(fileURLToPath(new URL(path, import.meta.url)), 'utf8')
  const block = new RegExp(`export \\{([^}]*)\\} from '\\.{1,2}\\/${module}'`).exec(source)
  if (block === null) throw new Error(`no ${module} re-export block found in ${path}`)
  return block[1]
    .split(',')
    .map((name) => name.replace('type ', '').trim())
    .filter((name) => name !== '')
    .sort()
}

test('both entrypoints export the same value marker types', () => {
  t.deepEqual(exportsOf('../ts/worker/index.ts', 'types\\.js'), exportsOf('../ts/index.ts', 'types\\.js'))
})

test('every entrypoint exports the same class wrapper surface', () => {
  const expected = exportsOf('../ts/index.ts', 'classInstance\\.js')
  t.true(expected.includes('BaseWrapperOptions'))
  for (const path of ENTRYPOINTS) {
    t.deepEqual(exportsOf(path, 'classInstance\\.js'), expected, path)
  }
})
