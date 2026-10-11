// Host module tests shared by the native and wasm backends.
import { test } from 'vitest'
import { t } from './assertions.js'

import {
  type CheckoutOptions,
  ClassInstance,
  ExternalModule,
  type ExternalModules,
  FunctionSnapshot,
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
const modules: ExternalModules = { tools: new ExternalModule(tools) }

async function run(code: string, externalModules: ExternalModules, options: CheckoutOptions = {}): Promise<unknown> {
  await using session = await pool().checkout(options)
  return await session.feedRun(code, { externalModules })
}

test('import binds the host module', async () => {
  t.deepEqual(await run(code, modules), [3, 'ab', 3])
})

test('the module object', async () => {
  const code = 'import tools\nimport tools as m\n[tools.add is m.add, type(tools).__name__, hasattr(tools, "nope")]'
  t.deepEqual(await run(code, modules), [true, 'tools', false])
})

test('import errors', async () => {
  await t.throwsAsync(run('import nope', modules), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'nope'",
  })
  await t.throwsAsync(run('import tools', {}), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'tools'",
  })
  await t.throwsAsync(run('from tools import nope', modules), {
    instanceOf: MontyRuntimeError,
    message: "ImportError: cannot import name 'nope' from 'tools' (unknown location)",
  })
})

test('a plain-object module is not callable and has no other methods', async () => {
  // Both calls suspend as methods on the module wrapper.
  await using session = await pool().checkout()
  await t.throwsAsync(session.feedRun('import tools\ntools()', { externalModules: modules }), {
    instanceOf: MontyRuntimeError,
    message: "TypeError: 'tools' object is not callable",
  })
  await t.throwsAsync(session.feedRun('import tools\ntools.nope()', { externalModules: modules }), {
    instanceOf: MontyRuntimeError,
    message: "AttributeError: 'tools' object has no attribute 'nope'",
  })
})

test('a restored plain-object module is still not callable', async () => {
  // Stable module IDs preserve type errors after restore without registering a new wrapper.
  let blob: Buffer
  {
    await using session = await pool().checkout()
    await session.feedRun('import tools', { externalModules: modules })
    blob = await session.dump()
  }
  await using session = await pool().checkout()
  await session.loadSession(blob)
  await t.throwsAsync(session.feedRun('tools()', { externalModules: modules }), {
    instanceOf: MontyRuntimeError,
    message: "TypeError: 'tools' object is not callable",
  })
  await t.throwsAsync(session.feedRun('tools.nope()', { externalModules: modules }), {
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
  t.is(await run('import tools\ntools.add(2, 3)', { tools: new ExternalModule(module) }), 5)
})

test('a ClassInstance module is read from each feed', async () => {
  class Tools {
    constructor(private readonly base: number) {}
    add(a: number): number {
      return this.base + a
    }
  }
  await using session = await pool().checkout()
  const feed = (base: number) => {
    const module = new ClassInstance(new Tools(base), { allowedMethods: ['add'] })
    return session.feedRun('import tools\ntools.add(1)', { externalModules: { tools: new ExternalModule(module) } })
  }
  t.is(await feed(10), 11)
  t.is(await feed(20), 21)
})

test('a plain-object module is read from each feed', async () => {
  class Tools {
    constructor(private readonly base: number) {}
    add(a: number): number {
      return this.base + a
    }
    remove(): string {
      return 'removed'
    }
  }
  const target = new Tools(10)
  await using session = await pool().checkout()
  const feed = (code: string, VERSION: number, allowedMethods: string[]) => {
    const inner = new ClassInstance(target, { allowedMethods, id: '11111111-1111-4111-8111-111111111111' })
    return session.feedRun(code, { externalModules: { tools: new ExternalModule({ VERSION, inner }) } })
  }
  // attributes and nested policies come from the feed that imports, not the first one
  t.deepEqual(await feed('import tools\n[tools.VERSION, tools.inner.remove()]', 1, ['add', 'remove']), [1, 'removed'])
  t.is(await feed('import tools\ntools.VERSION', 2, ['add']), 2)
  await t.throwsAsync(feed('import tools\ntools.inner.remove()', 2, ['add']), {
    instanceOf: MontyRuntimeError,
    message: "AttributeError: 'Tools' object has no attribute 'remove'",
  })
})

test('name-based calls respect module exposure', async () => {
  // Forged dotted names must not expose private attributes or bypass
  // ClassInstance's UUID dispatch and method policy.
  const privateProbe = () => 'hidden'
  Object.defineProperty(privateProbe, 'name', { value: 'tools._secret' })
  await using session = await pool().checkout()
  await t.throwsAsync(
    session.feedRun('import tools\nprobe()', {
      inputs: { probe: privateProbe },
      externalModules: { tools: new ExternalModule({ _secret: () => 'hidden' }) },
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
  // The earlier session retains its plain-object wrapper for `tools`.
  await using instanceSession = await pool().checkout()
  await t.throwsAsync(
    instanceSession.feedRun('import tools\nassert tools.reveal() == "hidden"\nprobe()', {
      inputs: { probe: instanceProbe },
      externalModules: { tools: new ExternalModule(new ClassInstance(new Tools(), { allowedMethods: ['reveal'] })) },
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
  t.deepEqual(await run(code, { tools: new ExternalModule({ first, second }) }), [1, 2])
})

test('resumeAuto answers imports', async () => {
  // Imports and stub requests both suspend as OS-call snapshots.
  const stubs = `${addStub}def concat(a: str, b: str) -> str: ...\nVERSION: int\n`
  const externalModules: ExternalModules = { tools: new ExternalModule(tools, { stubs }) }
  await using session = await pool().checkout({ typeCheck: true })
  let step = await session.feedStart(code, { externalModules })
  const calls: string[] = []
  while (!(step instanceof MontyComplete)) {
    const snapshot = step as FunctionSnapshot
    calls.push(`${snapshot.functionName}:${snapshot.isOsFunction}`)
    step = await snapshot.resumeAuto()
  }
  t.deepEqual(step.output, [3, 'ab', 3])
  t.deepEqual(calls, ['import_stubs:true', 'import:true', 'import:true', 'tools.add:false', 'tools.concat:false'])
})

test('module stubs type-check imports', async () => {
  const externalModules: ExternalModules = { tools: new ExternalModule(tools, { stubs: addStub }) }
  await using session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise' })
  await t.throwsAsync(session.feedRun("from tools import add\nadd('x', 2)", { externalModules }), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
  t.is(await session.feedRun('import tools\ntools.add(1, 2)', { externalModules }), 3)
  // Committed imports remain available to subsequent type checks.
  t.is(await session.feedRun('tools.add(3, 4)', { externalModules }), 7)
})

test('module stubs are fetched lazily', async () => {
  // Stub requests run the factory before type checking; later feeds retain the stubs.
  let calls = 0
  const factory = () => {
    calls += 1
    return new ExternalModule(tools, { stubs: addStub })
  }
  const externalModules: ExternalModules = { tools: factory }
  await using session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise' })
  t.is(await session.feedRun('1', { externalModules }), 1)
  t.is(calls, 0)
  await t.throwsAsync(session.feedRun("import tools\ntools.add('x', 2)", { externalModules }), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
  t.is(calls, 1)
  t.is(await session.feedRun('import tools\ntools.add(1, 2)', { externalModules }), 3)
  // One call for the failed feed's stub request, another for the next feed's import.
  t.is(calls, 2)
})

test('a module named __proto__ keeps its stub', async () => {
  const externalModules: ExternalModules = { ['__proto__']: new ExternalModule(tools, { stubs: addStub }) }
  await using session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise' })
  await t.throwsAsync(session.feedRun("import __proto__\n__proto__.add('x', 2)", { externalModules }), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:15: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
})

test('the os handler never sees a module ask', async () => {
  const externalModules: ExternalModules = { tools: new ExternalModule(tools, { stubs: addStub }) }
  const os = (name: string) => {
    throw new Error(`unexpected OS call ${name}`)
  }
  await using session = await pool().checkout({ typeCheck: true })
  t.is(await session.feedRun('import tools\ntools.add(1, 2)', { externalModules, os }), 3)
})

test('async factories are awaited together for a stubs ask', async () => {
  let release: () => void = () => {}
  const ready = new Promise<void>((resolve) => {
    release = resolve
  })
  const externalModules: ExternalModules = {
    tools: async () => {
      await ready
      return new ExternalModule(tools, { stubs: addStub })
    },
    other: async () => {
      release()
      return new ExternalModule({ VERSION: 2 }, { stubs: 'VERSION: int\n' })
    },
  }
  await using session = await pool().checkout({ typeCheck: true })
  t.is(await session.feedRun('import tools, other\ntools.add(other.VERSION, 1)', { externalModules }), 3)
})

test('a module without stubs does not type-check', async () => {
  await using session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise' })
  await t.throwsAsync(session.feedRun('import tools', { externalModules: modules }), {
    instanceOf: MontyTypingError,
    message: 'TypeError: main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`',
  })
  // Stub requests are cached per session, including missing stubs.
  await t.throwsAsync(
    session.feedRun('import tools', { externalModules: { tools: new ExternalModule(tools, { stubs: addStub }) } }),
    {
      instanceOf: MontyTypingError,
      message: 'TypeError: main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`',
    },
  )
})

test('module stubs ride in a dump', async () => {
  let blob: Buffer
  {
    await using session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise' })
    const externalModules: ExternalModules = { tools: new ExternalModule(tools, { stubs: addStub }) }
    t.is(await session.feedRun('import tools\ntools.add(1, 2)', { externalModules }), 3)
    blob = await session.dump()
  }
  // Dumps retain type checking, stubs and imports; the host must supply module values again.
  await using session = await pool().checkout()
  await session.loadSession(blob)
  await t.throwsAsync(session.feedRun("tools.add('x', 2)", { externalModules: modules }), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:1:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
  t.is(await session.feedRun('tools.add(3, 4)', { externalModules: modules }), 7)
})

test('invalid module names are rejected', async () => {
  await using session = await pool().checkout()
  // Bundled module imports never reach the host, even with a supplied stub.
  await t.throwsAsync(session.feedRun('1', { externalModules: { json: new ExternalModule({}, { stubs: '' }) } }), {
    instanceOf: TypeError,
    message: 'externalModules.json is provided by the sandbox and cannot be replaced',
  })
  await t.throwsAsync(session.feedRun('1', { externalModules: { 'pkg.tools': new ExternalModule(tools) } }), {
    instanceOf: TypeError,
    message: 'externalModules.pkg.tools is not a valid module name',
  })
  await t.throwsAsync(session.feedRun('1', { externalModules: { for: new ExternalModule(tools) } }), {
    instanceOf: TypeError,
    message: 'externalModules.for is not a valid module name',
  })
  await t.throwsAsync(session.feedRun('1', { externalModules: { json: new ExternalModule(tools) } }), {
    instanceOf: TypeError,
    message: 'externalModules.json is provided by the sandbox and cannot be replaced',
  })
  t.is(await session.feedRun('1 + 1'), 2)
})

test('every bundled module name is refused', async () => {
  // Compare the binding's independent list against the sandbox's actual modules.
  await using session = await pool().checkout()
  const names = (await session.feedRun('import sys\nlist(sys.builtin_module_names)')) as string[]
  t.true(names.includes('json'))
  for (const name of names) {
    await t.throwsAsync(session.feedRun('1', { externalModules: { [name]: new ExternalModule({}) } }), {
      message: `externalModules.${name} is provided by the sandbox and cannot be replaced`,
    })
  }
})

test('a module the type checker alone provides is served without a stub', async () => {
  t.is(await run('import abc\nabc.VERSION', { abc: new ExternalModule({ VERSION: 1 }) }), 1)
  // Host stubs cannot replace typeshed's `abc` stub.
  const abc = new ExternalModule({ VERSION: 1 }, { stubs: '' })
  await t.throwsAsync(run('import abc\nabc.VERSION', { abc }, { typeCheck: true, typeCheckFormat: 'concise' }), {
    instanceOf: MontyTypingError,
    message: 'TypeError: main.py:2:1: error[unresolved-attribute] Module `abc` has no member `VERSION`',
  })
})

test('submodules are attributes of their parent', async () => {
  const sub = new ExternalModule({ add: tools.add, VERSION: 2 })
  const pkg = new ExternalModule({ VERSION: 1, sub })
  await using session = await pool().checkout()
  const code = [
    'import pkg.sub',
    'import pkg.sub as s',
    'from pkg import sub',
    'from pkg.sub import add, VERSION',
    '[pkg.VERSION, pkg.sub.VERSION, s.add(1, 2), sub.add(3, 4), add(5, 6), VERSION, type(pkg.sub).__name__]',
  ].join('\n')
  t.deepEqual(await session.feedRun(code, { externalModules: { pkg } }), [1, 2, 3, 7, 11, 2, 'pkg.sub'])
  await t.throwsAsync(session.feedRun('import pkg.nope', { externalModules: { pkg } }), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'pkg.nope'",
  })
  await t.throwsAsync(session.feedRun('from pkg.sub.deeper import x', { externalModules: { pkg } }), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'pkg.sub.deeper'",
  })
  await t.throwsAsync(session.feedRun('import pkg.sub\npkg.sub()', { externalModules: { pkg } }), {
    instanceOf: MontyRuntimeError,
    message: "TypeError: 'pkg.sub' object is not callable",
  })
})

test('a ClassInstance submodule', async () => {
  class Tools {
    add(a: number, b: number): number {
      return a + b
    }
  }
  const sub = new ExternalModule(new ClassInstance(new Tools(), { allowedMethods: ['add'] }))
  t.is(await run('from pkg import sub\nsub.add(2, 3)', { pkg: new ExternalModule({ sub }) }), 5)
})

test('submodule stubs type-check as a package', async () => {
  const sub = new ExternalModule({ add: tools.add }, { stubs: addStub })
  const pkg = new ExternalModule({ VERSION: 1, sub }, { stubs: 'VERSION: int\n' })
  await using session = await pool().checkout({ typeCheck: true, typeCheckFormat: 'concise' })
  const code = 'import pkg.sub\nfrom pkg import VERSION\npkg.sub.add(1, VERSION)'
  t.is(await session.feedRun(code, { externalModules: { pkg } }), 2)
  await t.throwsAsync(session.feedRun("from pkg.sub import add\nadd('x', 2)", { externalModules: { pkg } }), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
})

test('invalid submodules are rejected', async () => {
  await using session = await pool().checkout()
  const feed = (pkg: ExternalModule) => session.feedRun('1', { externalModules: { pkg } })
  await t.throwsAsync(feed(new ExternalModule({ sub: new ExternalModule((() => tools) as never) })), {
    instanceOf: TypeError,
    message: 'externalModules.pkg.sub.module is function, not a plain object or ClassInstance',
  })
  await t.throwsAsync(feed(new ExternalModule({ '1sub': new ExternalModule(tools) })), {
    instanceOf: TypeError,
    message: "externalModules.pkg.module is a plain object with a key '1sub' that is not a valid identifier",
  })
  await t.throwsAsync(feed(new ExternalModule({ sub: new ExternalModule(tools, { stubs: 1 as never }) })), {
    instanceOf: TypeError,
    message: 'externalModules.pkg.sub.stubs must be a string',
  })
  const module: Record<string, unknown> = {}
  const pkg = new ExternalModule(module)
  module.sub = new ExternalModule({ pkg })
  await t.throwsAsync(feed(pkg), {
    instanceOf: TypeError,
    message: 'externalModules.pkg.sub.pkg is externalModules.pkg again: modules cannot nest cyclically',
  })
  await t.throwsAsync(feed(new ExternalModule({ for: new ExternalModule(tools) })), {
    instanceOf: TypeError,
    message: 'externalModules.pkg.for is not a valid module name',
  })
  await t.throwsAsync(feed(new ExternalModule({ json: new ExternalModule(tools) })), {
    instanceOf: TypeError,
    message: 'externalModules.pkg.json is provided by the sandbox and cannot be replaced',
  })
})

test('a getter handing back a module being imported is a cycle', async () => {
  const module: Record<string, unknown> = {}
  const pkg = new ExternalModule(module)
  Object.defineProperty(module, 'sub', { enumerable: true, get: () => pkg })
  await using session = await pool().checkout()
  await t.throwsAsync(session.feedRun('import pkg', { externalModules: { pkg } }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.pkg.sub is externalModules.pkg again: modules cannot nest cyclically',
  })
})

test('a submodule behind a getter is checked at the import', async () => {
  const lazy = {
    get sub(): ExternalModule {
      return new ExternalModule({ '1x': 1 })
    },
  }
  await using session = await pool().checkout()
  await t.throwsAsync(session.feedRun('import pkg', { externalModules: { pkg: new ExternalModule(lazy) } }), {
    instanceOf: MontyRuntimeError,
    message:
      "TypeError: externalModules.pkg.sub.module is a plain object with a key '1x' that is not a valid identifier",
  })
})

test('an entry must be an ExternalModule or a factory', async () => {
  await using session = await pool().checkout()
  await t.throwsAsync(session.feedRun('1', { externalModules: { tools: tools as never } }), {
    instanceOf: TypeError,
    message: 'externalModules.tools must be an ExternalModule or a function returning one',
  })
  await t.throwsAsync(
    session.feedRun('1', { externalModules: { tools: new ExternalModule(tools, { stubs: 1 as never }) } }),
    {
      instanceOf: TypeError,
      message: 'externalModules.tools.stubs must be a string',
    },
  )
})

test('module functions keep the module as their receiver', async () => {
  const counter = {
    count: 0,
    inc() {
      return ++this.count
    },
  }
  t.is(await run('import counter\ncounter.inc()\ncounter.inc()', { counter: new ExternalModule(counter) }), 2)
})

test('each module is its own class, the same on every import', async () => {
  const code =
    'import a\nimport b\nimport a as c\n[type(a).__name__, type(b).__name__, type(a) is type(b), type(a) is type(c)]'
  t.deepEqual(await run(code, { a: new ExternalModule({ x: 1 }), b: new ExternalModule({ y: 2 }) }), [
    'a',
    'b',
    false,
    true,
  ])
})

test('a forged name with many dots is walked through the configured modules', async () => {
  // Stop at the first missing component, even with thousands remaining.
  const probe = () => 'hidden'
  const name = `${'a.'.repeat(5000)}f`
  Object.defineProperty(probe, 'name', { value: name })
  const externalModules: ExternalModules = {
    tools: new ExternalModule(tools),
    a: new ExternalModule({ a: new ExternalModule(tools) }),
  }
  await using session = await pool().checkout()
  await t.throwsAsync(session.feedRun('probe()', { inputs: { probe }, externalModules }), {
    instanceOf: MontyRuntimeError,
    message: `NameError: name '${name}' is not defined`,
  })
  t.is(await session.feedRun('from a.a import add\nadd(1, 2)', { externalModules }), 3)
})

test('a getter that throws at the call raises in the sandbox', async () => {
  let reads = 0
  const flaky = {
    // Read once by the import, again by the call.
    get add(): (a: number, b: number) => number {
      if (reads++ > 0) throw new Error('gone')
      return (a, b) => a + b
    },
  }
  await using session = await pool().checkout()
  await t.throwsAsync(
    session.feedRun('import flaky\nflaky.add(1, 2)', { externalModules: { flaky: new ExternalModule(flaky) } }),
    {
      instanceOf: MontyRuntimeError,
      message: 'RuntimeError: gone',
    },
  )
  t.is(await session.feedRun('1 + 1'), 2)
})

test('a dotted key of a plain-object module is rejected', async () => {
  // Each key must be one component of a host function's dotted path.
  await using session = await pool().checkout()
  const tools = new ExternalModule({ 'a.b': (a: number, b: number) => a + b })
  await t.throwsAsync(session.feedRun('import tools', { externalModules: { tools } }), {
    instanceOf: TypeError,
    message: "externalModules.tools.module is a plain object with a key 'a.b' that is not a valid identifier",
  })
})

test('a module factory runs once per feed, when first needed', async () => {
  let calls = 0
  const factory = () => {
    calls += 1
    return new ExternalModule(tools)
  }
  const externalModules: ExternalModules = { tools: factory }
  await using session = await pool().checkout()
  t.is(await session.feedRun('1', { externalModules }), 1)
  t.is(calls, 0)
  const code = 'import tools\nimport tools as t\n[tools.add(1, 2), t.VERSION]'
  t.deepEqual(await session.feedRun(code, { externalModules }), [3, 3])
  t.is(calls, 1)
  // Factory results are cached only for the current feed.
  t.is(await session.feedRun('import tools\ntools.VERSION', { externalModules }), 3)
  t.is(calls, 2)
})

test('an async module factory is awaited', async () => {
  let calls = 0
  const factory = async () => {
    calls += 1
    await new Promise((resolve) => setTimeout(resolve, 1))
    return new ExternalModule(tools)
  }
  const code = 'import tools\nimport tools as t\n[tools.add(1, 2), t.VERSION]'
  t.deepEqual(await run(code, { tools: factory }), [3, 3])
  t.is(calls, 1)
})

test('an awaited factory serves a call through a restored binding', async () => {
  // Restored attributes allow a call without re-importing; the call must await the factory.
  let blob: Buffer
  {
    const first = new ExternalModule({ add: tools.add, sub: (a: number, b: number) => a - b })
    await using session = await pool().checkout()
    await session.feedRun('import tools', { externalModules: { tools: first } })
    blob = await session.dump()
  }
  const factory = async (): Promise<ExternalModule> => {
    await new Promise((resolve) => setTimeout(resolve, 1))
    return new ExternalModule({ add: async (a: number, b: number) => a + b, sub: (a: number, b: number) => a - b })
  }
  await using session = await pool().checkout()
  await session.loadSession(blob)
  const code = '[await tools.add(1, 2), tools.sub(5, 3), await tools.add(3, 4)]'
  t.deepEqual(await session.feedRun(code, { externalModules: { tools: factory } }), [3, 2, 7])
})

test('a module may carry its own then attribute', async () => {
  // Only real Promises are awaited; `then` remains a module function.
  const withThen = new ExternalModule({ add: tools.add, then: () => 'nope' })
  t.is(await run('import tools\ntools.add(1, 2)', { tools: () => withThen }), 3)
  t.is(await run('import tools\ntools.then()', { tools: () => withThen }), 'nope')
})

test('a private getter is never read by an import', async () => {
  const module = {
    add: tools.add,
    get _secret(): never {
      throw new Error('read')
    },
  }
  t.is(await run('import tools\ntools.add(1, 2)', { tools: new ExternalModule(module) }), 3)
})

test('a module factory failure raises at the import', async () => {
  const boom = () => {
    throw new Error('no tools today')
  }
  const notAModule = () => 3 as never
  const nullModule = () => null as never
  const badValue = () => new ExternalModule(3 as never)
  await using session = await pool().checkout()
  const externalModules: ExternalModules = { boom, notAModule, nullModule, badValue }
  await t.throwsAsync(session.feedRun('import boom', { externalModules }), {
    instanceOf: MontyRuntimeError,
    message: 'RuntimeError: no tools today',
  })
  await t.throwsAsync(session.feedRun('import notAModule', { externalModules }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.notAModule() returned number, not an ExternalModule',
  })
  await t.throwsAsync(session.feedRun('import nullModule', { externalModules }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.nullModule() returned null, not an ExternalModule',
  })
  await t.throwsAsync(session.feedRun('import badValue', { externalModules }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.badValue.module is number, not a plain object or ClassInstance',
  })
  t.is(await session.feedRun('1 + 1'), 2)
})

test('a module must be a plain object or ClassInstance', async () => {
  // Class instances require an explicit exposure policy through ClassInstance.
  class Tools {
    add(a: number, b: number): number {
      return a + b
    }
  }
  const asModule = (value: unknown) => new ExternalModule(value as Record<string, unknown>)
  await using session = await pool().checkout()
  await t.throwsAsync(session.feedRun('import tools', { externalModules: { tools: asModule(new Tools()) } }), {
    instanceOf: TypeError,
    message: 'externalModules.tools.module is a Tools, not a plain object or ClassInstance',
  })
  await t.throwsAsync(session.feedRun('import tools', { externalModules: { tools: asModule(new Map()) } }), {
    instanceOf: TypeError,
    message: 'externalModules.tools.module is a Map, not a plain object or ClassInstance',
  })
  await t.throwsAsync(session.feedRun('import tools', { externalModules: { tools: () => asModule(new Tools()) } }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.tools.module is a Tools, not a plain object or ClassInstance',
  })
  // Module namespaces have null prototypes.
  const bare = Object.assign(Object.create(null) as Record<string, unknown>, tools)
  t.deepEqual(await session.feedRun(code, { externalModules: { tools: new ExternalModule(bare) } }), [3, 'ab', 3])
})

test('a module that fails to materialize raises at the import', async () => {
  const broken = {
    get boom(): number {
      throw new Error('nope')
    },
  }
  await t.throwsAsync(run('import broken', { broken: new ExternalModule(broken) }), {
    instanceOf: MontyRuntimeError,
    message: 'RuntimeError: nope',
  })
})
