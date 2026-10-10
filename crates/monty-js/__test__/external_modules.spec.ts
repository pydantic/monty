// `externalModules`: host modules the sandbox imports, paired with the module
// stubs that type-check them. Shared across the native and wasm backends.
import { test } from 'vitest'
import { t } from './assertions.js'

import {
  type CheckoutOptions,
  ClassInstance,
  type ExternalModules,
  FunctionSnapshot,
  type ModuleValue,
  MontyComplete,
  MontyRuntimeError,
  MontyTypingError,
} from '@pydantic/monty'
import { setupPool } from './helpers.js'

const { pool } = setupPool()

const tools = {
  add: (a: number, b: number) => a + b,
  concat: (a: string, kwargs: { b: string }) => a + kwargs.b,
  VERSION: 3,
}
const code = "import tools\nfrom tools import concat\n[tools.add(1, 2), concat('a', b='b'), tools.VERSION]"
const addStub = 'def add(a: int, b: int) -> int: ...\n'

/** Runs `code` in a fresh session checked out with `externalModules`. */
async function run(code: string, externalModules: ExternalModules, options: CheckoutOptions = {}): Promise<unknown> {
  await using session = await pool().checkout({ ...options, externalModules })
  return await session.feedRun(code)
}

test('import binds the host module', async () => {
  t.deepEqual(await run(code, { tools: { module: tools } }), [3, 'ab', 3])
})

test('the module object', async () => {
  const code = 'import tools\nimport tools as m\n[tools.add is m.add, type(tools).__name__, hasattr(tools, "nope")]'
  t.deepEqual(await run(code, { tools: { module: tools } }), [true, 'tools', false])
})

test('import errors', async () => {
  await t.throwsAsync(run('import nope', { tools: { module: tools } }), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'nope'",
  })
  await t.throwsAsync(run('import tools', {}), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'tools'",
  })
  await t.throwsAsync(run('from tools import nope', { tools: { module: tools } }), {
    instanceOf: MontyRuntimeError,
    message: "ImportError: cannot import name 'nope' from 'tools' (unknown location)",
  })
})

test('a plain-object module is not callable and has no other methods', async () => {
  // the sandbox routes `tools()` and `tools.nope()` to the host as method calls on
  // the module's stand-in, which answers as a value of that kind would
  await using session = await pool().checkout({ externalModules: { tools: { module: tools } } })
  await t.throwsAsync(session.feedRun('import tools\ntools()'), {
    instanceOf: MontyRuntimeError,
    message: "TypeError: 'tools' object is not callable",
  })
  await t.throwsAsync(session.feedRun('import tools\ntools.nope()'), {
    instanceOf: MontyRuntimeError,
    message: "AttributeError: 'tools' object has no attribute 'nope'",
  })
})

test('a restored plain-object module is still not callable', async () => {
  // the module's stand-in keeps the same id in every process, so a session restored
  // from a dump answers `tools()` as a value of its kind, not as a store miss
  let blob: Buffer
  {
    await using session = await pool().checkout({ externalModules: { tools: { module: tools } } })
    await session.feedRun('import tools')
    blob = await session.dump()
  }
  await using session = await pool().checkout({ externalModules: { tools: { module: tools } } })
  await session.loadSession(blob)
  await t.throwsAsync(session.feedRun('tools()'), {
    instanceOf: MontyRuntimeError,
    message: "TypeError: 'tools' object is not callable",
  })
  await t.throwsAsync(session.feedRun('tools.nope()'), {
    instanceOf: MontyRuntimeError,
    message: "AttributeError: 'tools' object has no attribute 'nope'",
  })
})

test('a ClassInstance module', async () => {
  class Tools {
    add(a: number, b: number): number {
      return a + b
    }
  }
  const module = new ClassInstance(new Tools(), { allowedMethods: ['add'] })
  t.is(await run('import tools\ntools.add(2, 3)', { tools: { module } }), 5)
})

test('name-based calls respect module exposure', async () => {
  // A `tools.<attr>` call reaches only what `import tools` exposed: never a private
  // name, and nothing on a ClassInstance module, whose methods route by uuid under
  // its policy. With the module imported, a host function input carrying such a
  // name (as a forged frame would) still finds nothing.
  const privateProbe = () => 'hidden'
  Object.defineProperty(privateProbe, 'name', { value: 'tools._secret' })
  {
    // scoped so the worker is back in the pool before the next checkout
    await using session = await pool().checkout({
      externalModules: { tools: { module: { _secret: () => 'hidden' } } },
    })
    await t.throwsAsync(session.feedRun('import tools\nprobe()', { inputs: { probe: privateProbe } }), {
      instanceOf: MontyRuntimeError,
      message: "NameError: name 'tools._secret' is not defined",
    })
  }
  class Tools {
    reveal(): string {
      return 'hidden'
    }
  }
  const instanceProbe = () => 'hidden'
  Object.defineProperty(instanceProbe, 'name', { value: 'tools.reveal' })
  await using instanceSession = await pool().checkout({
    externalModules: { tools: { module: new ClassInstance(new Tools(), { allowedMethods: ['reveal'] }) } },
  })
  await t.throwsAsync(
    instanceSession.feedRun('import tools\nassert tools.reveal() == "hidden"\nprobe()', {
      inputs: { probe: instanceProbe },
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
  t.deepEqual(await run(code, { tools: { module: { first, second } } }), [1, 2])
})

test('resumeAuto answers imports', async () => {
  const session = await pool().checkout({ externalModules: { tools: { module: tools } } })
  try {
    let step = await session.feedStart(code)
    while (!(step instanceof MontyComplete)) {
      step = await (step as FunctionSnapshot).resumeAuto()
    }
    t.deepEqual(step.output, [3, 'ab', 3])
  } finally {
    await session.close()
  }
})

test('module stubs type-check imports', async () => {
  const externalModules: ExternalModules = { tools: { module: tools, stubs: addStub } }
  await using session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise', externalModules })
  await t.throwsAsync(session.feedRun("from tools import add\nadd('x', 2)"), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
  t.is(await session.feedRun('import tools\ntools.add(1, 2)'), 3)
  // the import committed by that feed is still bound for the next check
  t.is(await session.feedRun('tools.add(3, 4)'), 7)
})

test('a module without stubs does not type-check', async () => {
  await using session = await pool().checkout({
    typeCheck: true,
    typeCheckFormat: 'concise',
    externalModules: { tools: { module: tools } },
  })
  await t.throwsAsync(session.feedRun('import tools'), {
    instanceOf: MontyTypingError,
    message: 'TypeError: main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`',
  })
})

test('module stubs ride in a dump', async () => {
  let blob: Buffer
  {
    await using session = await pool().checkout({
      typeCheck: true,
      typeCheckFormat: 'concise',
      externalModules: { tools: { module: tools, stubs: addStub } },
    })
    t.is(await session.feedRun('import tools\ntools.add(1, 2)'), 3)
    blob = await session.dump()
  }
  // the dump brings its own type checking, stubs and committed import; the module
  // itself is host state, so the restoring checkout supplies it again
  await using session = await pool().checkout({ externalModules: { tools: { module: tools } } })
  await session.loadSession(blob)
  await t.throwsAsync(session.feedRun("tools.add('x', 2)"), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:1:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
  t.is(await session.feedRun('tools.add(3, 4)'), 7)
})

test('invalid module names are rejected', async () => {
  // the native binding refuses before dialing; the wasm child on `Configure`
  await t.throwsAsync(pool().checkout({ externalModules: { json: { module: {}, stubs: '' } } }), {
    message: /module "json" is provided by the sandbox or its type checker and cannot be replaced/,
  })
})

test('an entry must be an object with a module property', async () => {
  const asModule = (value: unknown) => value as ExternalModules[string]
  await t.throwsAsync(pool().checkout({ externalModules: { tools: asModule(tools) } }), {
    instanceOf: TypeError,
    message: 'externalModules.tools must be an object with a module property',
  })
  await t.throwsAsync(pool().checkout({ externalModules: { tools: asModule({ module: tools, stubs: 1 }) } }), {
    instanceOf: TypeError,
    message: 'externalModules.tools.stubs must be a string',
  })
})

test('module functions keep the module as their receiver', async () => {
  const counter = {
    count: 0,
    inc() {
      return ++this.count
    },
  }
  t.is(await run('import counter\ncounter.inc()\ncounter.inc()', { counter: { module: counter } }), 2)
})

test('each module is its own class, the same on every import', async () => {
  const code =
    'import a\nimport b\nimport a as c\n[type(a).__name__, type(b).__name__, type(a) is type(b), type(a) is type(c)]'
  t.deepEqual(await run(code, { a: { module: { x: 1 } }, b: { module: { y: 2 } } }), ['a', 'b', false, true])
})

test('a dotted module name', async () => {
  // the module's own name may hold dots; the attribute a call names never does
  t.is(await run('from pkg.tools import add\nadd(1, 2)', { 'pkg.tools': { module: tools } }), 3)
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
  await using session = await pool().checkout({ externalModules: { flaky: { module: flaky } } })
  await t.throwsAsync(session.feedRun('import flaky\nflaky.add(1, 2)'), {
    instanceOf: MontyRuntimeError,
    message: 'RuntimeError: gone',
  })
  // the session is still usable
  t.is(await session.feedRun('1 + 1'), 2)
})

test('a dotted key of a plain-object module', async () => {
  const code = "import tools\ngetattr(tools, 'a.b')(1, 2)"
  t.is(await run(code, { tools: { module: { 'a.b': (a: number, b: number) => a + b } } }), 3)
})

test('a module factory runs once per session, at the first import', async () => {
  let calls = 0
  const factory = () => {
    calls += 1
    return tools
  }
  await using session = await pool().checkout({ externalModules: { tools: { module: factory } } })
  t.is(await session.feedRun('1'), 1)
  t.is(calls, 0)
  const code = 'import tools\nimport tools as t\n[tools.add(1, 2), t.VERSION]'
  t.deepEqual(await session.feedRun(code), [3, 3])
  t.is(calls, 1)
  // the result stands for the module for the rest of the session
  t.is(await session.feedRun('import tools\ntools.VERSION'), 3)
  t.is(calls, 1)
})

test('an async module factory is awaited', async () => {
  let calls = 0
  const factory = async () => {
    calls += 1
    await new Promise((resolve) => setTimeout(resolve, 1))
    return tools
  }
  const code = 'import tools\nimport tools as t\n[tools.add(1, 2), t.VERSION]'
  t.deepEqual(await run(code, { tools: { module: factory } }), [3, 3])
  t.is(calls, 1)
})

test('an awaited factory serves a call through a restored binding', async () => {
  // `tools` was bound by a feed of the dumped session, whose module is gone with
  // it; the restored session first needs the module for a call through that
  // binding (the attributes sent with it survive the dump), so the factory is
  // awaited on the call path
  let blob: Buffer
  {
    const first = { add: tools.add, sub: (a: number, b: number) => a - b }
    await using session = await pool().checkout({ externalModules: { tools: { module: first } } })
    await session.feedRun('import tools')
    blob = await session.dump()
  }
  const factory = async (): Promise<ModuleValue> => {
    await new Promise((resolve) => setTimeout(resolve, 1))
    return { add: async (a: number, b: number) => a + b, sub: (a: number, b: number) => a - b }
  }
  await using session = await pool().checkout({ externalModules: { tools: { module: factory } } })
  await session.loadSession(blob)
  t.deepEqual(await session.feedRun('[await tools.add(1, 2), tools.sub(5, 3), await tools.add(3, 4)]'), [3, 2, 7])
})

test('a module may carry its own then attribute', async () => {
  // only a real Promise from a factory is awaited, so `then` stays a module function
  const withThen = { add: tools.add, then: () => 'nope' }
  t.is(await run('import tools\ntools.add(1, 2)', { tools: { module: () => withThen } }), 3)
  t.is(await run('import tools\ntools.then()', { tools: { module: () => withThen } }), 'nope')
})

test('a private getter is never read by an import', async () => {
  const module = {
    add: tools.add,
    get _secret(): never {
      throw new Error('read')
    },
  }
  t.is(await run('import tools\ntools.add(1, 2)', { tools: { module } }), 3)
})

test('a module factory failure raises at the import', async () => {
  const boom = () => {
    throw new Error('no tools today')
  }
  const notAModule = () => 3 as unknown as Record<string, unknown>
  const nullModule = () => null as unknown as Record<string, unknown>
  await using session = await pool().checkout({
    externalModules: { boom: { module: boom }, notAModule: { module: notAModule }, nullModule: { module: nullModule } },
  })
  await t.throwsAsync(session.feedRun('import boom'), {
    instanceOf: MontyRuntimeError,
    message: 'RuntimeError: no tools today',
  })
  await t.throwsAsync(session.feedRun('import notAModule'), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.notAModule.module() returned number, not a plain object or ClassInstance',
  })
  await t.throwsAsync(session.feedRun('import nullModule'), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.nullModule.module() returned null, not a plain object or ClassInstance',
  })
  // the session is still usable
  t.is(await session.feedRun('1 + 1'), 2)
})

test('a module must be a plain object or ClassInstance', async () => {
  // a plain object's own keys name exactly what the sandbox may reach; an instance
  // of another class would expose whatever its prototype chain carries
  class Tools {
    add(a: number, b: number): number {
      return a + b
    }
  }
  const asModule = (value: unknown) => value as Record<string, unknown>
  await t.throwsAsync(run('import tools', { tools: { module: asModule(new Tools()) } }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.tools.module is a Tools, not a plain object or ClassInstance',
  })
  await t.throwsAsync(run('import tools', { tools: { module: asModule(new Map()) } }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.tools.module is a Map, not a plain object or ClassInstance',
  })
  await t.throwsAsync(run('import tools', { tools: { module: () => asModule(new Tools()) } }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.tools.module() returned a Tools, not a plain object or ClassInstance',
  })
  // a null-prototype object, as a module namespace is, counts as plain
  const bare = Object.assign(Object.create(null) as Record<string, unknown>, tools)
  t.deepEqual(await run(code, { tools: { module: bare } }), [3, 'ab', 3])
})

test('a module that fails to materialize raises at the import', async () => {
  const broken = {
    get boom(): number {
      throw new Error('nope')
    },
  }
  await t.throwsAsync(run('import broken', { broken: { module: broken } }), {
    instanceOf: MontyRuntimeError,
    message: 'RuntimeError: nope',
  })
})
