// `externalModules`: host modules the sandbox imports, and the module stubs
// that type-check them. Shared across the native and wasm backends.
import { test } from 'vitest'
import { t } from './assertions.js'

import { ClassInstance, FunctionSnapshot, MontyComplete, MontyRuntimeError, MontyTypingError } from '@pydantic/monty'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

const tools = {
  add: (a: number, b: number) => a + b,
  concat: (a: string, kwargs: { b: string }) => a + kwargs.b,
  VERSION: 3,
}
const code = "import tools\nfrom tools import concat\n[tools.add(1, 2), concat('a', b='b'), tools.VERSION]"

test('import binds the host module', async () => {
  t.deepEqual(await run(code, { externalModules: { tools } }), [3, 'ab', 3])
})

test('the module object', async () => {
  const code = 'import tools\nimport tools as m\n[tools.add is m.add, type(tools).__name__, hasattr(tools, "nope")]'
  t.deepEqual(await run(code, { externalModules: { tools } }), [true, 'tools', false])
})

test('import errors', async () => {
  await t.throwsAsync(run('import nope', { externalModules: { tools } }), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'nope'",
  })
  await t.throwsAsync(run('import tools'), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'tools'",
  })
  await t.throwsAsync(run('from tools import nope', { externalModules: { tools } }), {
    instanceOf: MontyRuntimeError,
    message: "ImportError: cannot import name 'nope' from 'tools' (unknown location)",
  })
})

test('a ClassInstance module', async () => {
  class Tools {
    add(a: number, b: number): number {
      return a + b
    }
  }
  const module = new ClassInstance(new Tools(), { allowedMethods: ['add'] })
  t.is(await run('import tools\ntools.add(2, 3)', { externalModules: { tools: module } }), 5)
})

test('name-based calls respect module exposure', async () => {
  // A `tools.<attr>` call reaches only what `import tools` exposed: never a private
  // name, and nothing on a ClassInstance module, whose methods route by uuid under
  // its policy. With the module imported, a host function input carrying such a
  // name (as a forged frame would) still finds nothing.
  const privateProbe = () => 'hidden'
  Object.defineProperty(privateProbe, 'name', { value: 'tools._secret' })
  await t.throwsAsync(
    run('import tools\nprobe()', {
      inputs: { probe: privateProbe },
      externalModules: { tools: { _secret: () => 'hidden' } },
    }),
    { instanceOf: MontyRuntimeError, message: "NameError: name 'tools._secret' is not defined" },
  )
  class Tools {
    reveal(): string {
      return 'hidden'
    }
  }
  const instanceProbe = () => 'hidden'
  Object.defineProperty(instanceProbe, 'name', { value: 'tools.reveal' })
  await t.throwsAsync(
    run('import tools\nassert tools.reveal() == "hidden"\nprobe()', {
      inputs: { probe: instanceProbe },
      externalModules: { tools: new ClassInstance(new Tools(), { allowedMethods: ['reveal'] }) },
    }),
    { instanceOf: MontyRuntimeError, message: "NameError: name 'tools.reveal' is not defined" },
  )
})

test('async tools run concurrently', async () => {
  let release: () => void = () => {}
  const ready = new Promise<void>((resolve) => {
    release = resolve
  })
  const first = async () => {
    await ready
    return 1
  }
  const second = async () => {
    release()
    return 2
  }
  const code = 'import asyncio\nimport tools\nfrom tools import second\nawait asyncio.gather(tools.first(), second())'
  t.deepEqual(await run(code, { externalModules: { tools: { first, second } } }), [1, 2])
})

test('resumeAuto answers imports', async () => {
  const session = await pool().checkout()
  try {
    let step = await session.feedStart(code, { externalModules: { tools } })
    while (!(step instanceof MontyComplete)) {
      step = await (step as FunctionSnapshot).resumeAuto()
    }
    t.deepEqual(step.output, [3, 'ab', 3])
  } finally {
    await session.close()
  }
})

test('module stubs type-check imports and come back from getStubs', async () => {
  const typeCheckModuleStubs = { tools: 'def add(a: int, b: int) -> int: ...\n' }
  const session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise', typeCheckModuleStubs })
  try {
    t.deepEqual(await session.getStubs(), typeCheckModuleStubs)
    await t.throwsAsync(session.feedRun("from tools import add\nadd('x', 2)", { externalModules: { tools } }), {
      instanceOf: MontyTypingError,
      message:
        'TypeError: main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
    })
    t.is(await session.feedRun('import tools\ntools.add(1, 2)', { externalModules: { tools } }), 3)
    // the import committed by that feed is still bound for the next check
    t.is(await session.feedRun('tools.add(3, 4)', { externalModules: { tools } }), 7)
  } finally {
    await session.close()
  }
})

test('invalid module names are rejected', async () => {
  // the native binding refuses before dialing; the wasm child on `Configure`
  await t.throwsAsync(pool().checkout({ typeCheckModuleStubs: { json: '' } }), {
    message: /module "json" is provided by the sandbox or its type checker and cannot be replaced/,
  })
})

test('module functions keep the module as their receiver', async () => {
  const counter = {
    count: 0,
    inc() {
      return ++this.count
    },
  }
  t.is(await run('import counter\ncounter.inc()\ncounter.inc()', { externalModules: { counter } }), 2)
})

test('each module is its own class, the same on every import', async () => {
  const code =
    'import a\nimport b\nimport a as c\n[type(a).__name__, type(b).__name__, type(a) is type(b), type(a) is type(c)]'
  t.deepEqual(await run(code, { externalModules: { a: { x: 1 }, b: { y: 2 } } }), ['a', 'b', false, true])
})

test('a dotted module name', async () => {
  // the module's own name may hold dots; the attribute a call names never does
  t.is(await run('from pkg.tools import add\nadd(1, 2)', { externalModules: { 'pkg.tools': tools } }), 3)
})

test('a getter that throws at the call raises in the sandbox', async () => {
  let reads = 0
  const flaky = {
    // read once by the import, again by the call
    get add(): (a: number, b: number) => number {
      if (reads++ > 0) throw new Error('gone')
      return (a, b) => a + b
    },
  }
  await using session = await pool().checkout()
  await t.throwsAsync(session.feedRun('import flaky\nflaky.add(1, 2)', { externalModules: { flaky } }), {
    instanceOf: MontyRuntimeError,
    message: 'RuntimeError: gone',
  })
  // the session is still usable
  t.is(await session.feedRun('1 + 1'), 2)
})

test('a dotted key of a plain-object module', async () => {
  const code = "import tools\ngetattr(tools, 'a.b')(1, 2)"
  t.is(await run(code, { externalModules: { tools: { 'a.b': (a: number, b: number) => a + b } } }), 3)
})

test('a module that fails to materialize raises at the import', async () => {
  const broken = {
    get boom(): number {
      throw new Error('nope')
    },
  }
  await t.throwsAsync(run('import broken', { externalModules: { broken } }), {
    instanceOf: MontyRuntimeError,
    message: 'RuntimeError: nope',
  })
})
