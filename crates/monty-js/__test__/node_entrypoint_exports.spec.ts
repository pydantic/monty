import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { test } from 'vitest'

import { t } from './assertions.js'

test('both entrypoints export the same value marker types', () => {
  // `@pydantic/monty` and `@pydantic/monty/wasm` are the same API over two
  // transports, so a type reaching only one of them is a packaging bug — the
  // wasm entrypoint re-exports `../types.js` by hand and has drifted before.
  const exportsOf = (path: string): string[] => {
    const source = readFileSync(fileURLToPath(new URL(path, import.meta.url)), 'utf8')
    const block = /export \{([^}]*)\} from '\.{1,2}\/types\.js'/.exec(source)
    if (block === null) throw new Error(`no types.js re-export block found in ${path}`)
    return block[1]
      .split(',')
      .map((name) => name.replace('type ', '').trim())
      .filter((name) => name !== '')
      .sort()
  }
  t.deepEqual(exportsOf('../ts/worker/index.ts'), exportsOf('../ts/index.ts'))
})
