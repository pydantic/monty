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

test('module stubs type-check imports and come back from getTypes', async () => {
  const typeCheckModuleStubs = { tools: 'def add(a: int, b: int) -> int: ...\n' }
  const session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise', typeCheckModuleStubs })
  try {
    t.deepEqual(await session.getTypes(), typeCheckModuleStubs)
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

test('invalid module stub names are rejected', async () => {
  // the native binding refuses before dialing; the wasm child on `Configure`
  await t.throwsAsync(pool().checkout({ typeCheckModuleStubs: { json: '' } }), {
    message: /module "json" is provided by the sandbox and cannot take a stub/,
  })
})
