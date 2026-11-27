// Values that exceed a 32-bit `usize` reach the interpreter only here.
//
// Interpreter semantics normally belong in `crates/monty/test_cases`, but the
// wasm worker is Monty's one 32-bit target: an `int` above `usize::MAX` yet
// inside `i64` — `2**40` — is a conversion no 64-bit build ever performs, so
// this is the only harness that can reach the case at all. The `i64` overflow
// these mirror is covered on every target by `collections__deque.py`.
//
// Each must raise the `OverflowError` a 32-bit CPython gives, not trap the
// instance. The two messages differ because CPython's do: `deque` converts
// `maxlen` with `PyLong_AsSsize_t`, `bytes` its count with `PyNumber_AsSsize_t`.

import { test } from 'vitest'

import { t } from './assertions.js'
import { Monty, MontyRuntimeError } from '@pydantic/monty/wasm'

// each case is over a 32-bit `ssize_t` but under `i64::MAX`: `2**40`, or an io write ending past `2**31 - 1`
const OVER_32_BIT = [
  ['deque maxlen', 'from collections import deque\ndeque([], 2**40)', 'Python int too large to convert to C ssize_t'],
  ['bytes count', 'bytes(2**40)', "cannot fit 'int' into an index-sized integer"],
  ['BytesIO seek', 'import io\nio.BytesIO().seek(2**40)', 'Python int too large to convert to C ssize_t'],
  [
    'BytesIO relative seek',
    'import io\nio.BytesIO().seek(-(2**40), 1)',
    'Python int too large to convert to C ssize_t',
  ],
  ['BytesIO seek end', 'import io\nb = io.BytesIO()\nb.seek(2**31 - 1)\nb.seek(1, 1)', 'new position too large'],
  ['BytesIO read size', "import io\nio.BytesIO(b'a').read(2**40)", "cannot fit 'int' into an index-sized integer"],
  ['StringIO read size', "import io\nio.StringIO('a').read(2**40)", "cannot fit 'int' into an index-sized integer"],
  ['BytesIO truncate size', "import io\nio.BytesIO(b'a').truncate(2**40)", 'Python int too large to convert to C long'],
  ['BytesIO write end', "import io\nb = io.BytesIO()\nb.seek(2**31 - 1)\nb.write(b'xx')", 'new buffer size too large'],
  ['StringIO write end', "import io\ns = io.StringIO()\ns.seek(2**31 - 1)\ns.write('xx')", 'new position too large'],
] as const

for (const [name, code, message] of OVER_32_BIT) {
  test(`an over-32-bit ${name} raises rather than trapping`, async () => {
    await using pool = await Monty.create()
    await using session = await pool.checkout({})
    const error = await t.throwsAsync(() => session.feedRun(code), { instanceOf: MontyRuntimeError })
    t.is(error.exception.typeName, 'OverflowError')
    t.is(error.exception.message, message)
    // the instance survived: a trap would have taken the session with it
    t.is(await session.feedRun('1 + 1'), 2)
  })
}
