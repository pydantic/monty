import { test } from 'vitest'

import * as root from '@pydantic/monty'
import * as node from '@pydantic/monty/node'
import * as wasm from '@pydantic/monty/wasm'
import * as shared from '../dist/shared.js'
import { t } from './assertions.js'

// Compare public modules rather than parsing the spelling of their re-exports.
test('every entrypoint exports the same shared values', () => {
  for (const entry of [root, node, wasm]) {
    for (const name of Object.keys(shared) as Array<keyof typeof shared>) {
      t.is(entry[name], shared[name], name)
    }
  }
})

test('the WASM entry exports no implementation machinery', () => {
  t.deepEqual(Object.keys(wasm).sort(), [...Object.keys(shared), 'Monty', 'createWorkerPool', 'loadModule'].sort())
})
