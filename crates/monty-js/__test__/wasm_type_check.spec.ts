// Type checking across feeds in the wasm worker, driven in Node so it needs no
// browser.
//
// A feed whose check fails writes its source file twice, and wasm's clock is
// only millisecond-granular, so the second write used to be invisible to Salsa
// and the next feed reused the stale result.

import { test } from 'vitest'

// everything comes from `/wasm`, which re-exports the error classes: importing
// them from `@pydantic/monty` would pull in the napi loader, and this suite
// runs where only the wasm module has been built
import { Monty, MontyTypingError } from '@pydantic/monty/wasm'

import { t } from './assertions.js'
import { skipIfBrowser } from './env.js'

// a single pass only sometimes lands both writes in one millisecond
const ATTEMPTS = 15

test('a feed after a failed type check leaves the worker alive', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
    const session = await pool.checkout({ typeCheck: true })
    await session.feedRun('x = 1')

    // the message is the first diagnostic, here the narrowed reassignment
    const error = await t.throwsAsync(() => session.feedRun('x = 2\n"hello" + 1'), {
      instanceOf: MontyTypingError,
    })
    t.is(
      error.message,
      'TypeError: error[invalid-assignment]: Object of type `Literal[2]` is not assignable to `Literal[1]`',
    )

    // the rejected snippet left neither a binding nor a stale type-check file
    t.is(await session.feedRun('x'), 1)
    await session.close()
  }
  await pool.close()
})

test('a repeated failing feed reports the same diagnostic every time', async (ctx) => {
  skipIfBrowser(ctx)
  const pool = await Monty.create()
  const session = await pool.checkout({ typeCheck: true })
  await session.feedRun('x = 1')
  for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
    // rendering reads the db text back, so a stale file misquotes the source
    const error = await t.throwsAsync(() => session.feedRun('"hello" + 1'), {
      instanceOf: MontyTypingError,
    })
    t.is(
      error.display(),
      [
        'error[unsupported-operator]: Unsupported `+` operation',
        ' --> main.py:1:1',
        '  |',
        '1 | "hello" + 1',
        '  | -------^^^-',
        '  | |         |',
        '  | |         Has type `Literal[1]`',
        '  | Has type `Literal["hello"]`',
        '  |',
        '',
        '',
      ].join('\n'),
    )
  }
  await session.close()
  await pool.close()
})
