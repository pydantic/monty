// One `Print` event can carry runs from both streams, which the worker
// expands into one callback per run. Over the wasm transport that expansion
// happens inside the component, so the labels are worth checking there too.

import { test } from 'vitest'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'
import { Monty } from '@pydantic/monty/wasm'

test('stdout and stderr keep their labels and order over the wasm transport', async (ctx) => {
  skipIfBrowser(ctx)
  await using pool = await Monty.create()
  await using session = await pool.checkout({})

  const received: [string, string][] = []
  await session.feedRun("import sys\nprint('a')\nprint('b', file=sys.stderr)\nprint('c')", {
    printCallback: (stream, text) => received.push([stream, text]),
  })

  t.deepEqual(received, [
    ['stdout', 'a\n'],
    ['stderr', 'b\n'],
    ['stdout', 'c\n'],
  ])
})
