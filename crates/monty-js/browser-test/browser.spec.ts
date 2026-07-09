import { expect, test } from 'vitest'

import { Monty, MontyRepl } from '../ts/wasm.ts'

test('Monty wasm runs in a browser', () => {
  const add = new Monty('1 + 2')
  const ext = new Monty('add_ints(2, 3)')
  const repl = new MontyRepl()

  repl.feed('x = 2')

  expect({
    add: add.run(),
    ext: ext.run({ externalLookup: { add_ints: (a: number, b: number) => a + b } }),
    repl: repl.feed('x + 2'),
    crossOriginIsolated: globalThis.crossOriginIsolated,
  }).toMatchObject({
    add: 3,
    ext: 5,
    repl: 4,
    crossOriginIsolated: true,
  })
})
