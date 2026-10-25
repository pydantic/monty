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
const toolsModule = new ExternalModule(tools)
const modules: ExternalModules = { tools: toolsModule }

/** A session whose feeds may import `externalModules`. */
function checkout(externalModules: ExternalModules, options: CheckoutOptions = {}) {
  return pool().checkout({ ...options, externalModules })
}

async function run(code: string, externalModules: ExternalModules, options: CheckoutOptions = {}): Promise<unknown> {
  await using session = await checkout(externalModules, options)
  return await session.feedRun(code)
}

test('import binds the host module', async () => {
  t.deepEqual(await run(code, modules), [3, 'ab', 3])
})

test('the module object', async () => {
  const code =
    'import tools\nimport tools as m\n[tools.add is m.add, type(tools).__name__, hasattr(tools, "nope"), repr(tools)]'
  t.deepEqual(await run(code, modules), [true, 'module', false, "<module 'tools'>"])
})

test('the module crosses back with its id', async () => {
  await using session = await checkout(modules)
  const returned = (await session.feedRun('import tools\ntools')) as ExternalModule
  t.true(returned instanceof ExternalModule)
  t.is(returned.id, toolsModule.id)
  t.is(returned.stubs, undefined)
  t.deepEqual(Object.keys(returned.attrs), ['add', 'concat', 'VERSION'])
  t.is(returned.attrs.VERSION, 3)
  t.is(await session.feedRun('import tools\nimport tools as m\ntools is m'), false)
})

test('a module is not a value', async () => {
  await using session = await checkout(modules)
  await t.throwsAsync(session.feedRun('x', { inputs: { x: new ExternalModule({}) } }), {
    instanceOf: TypeError,
    message: 'an ExternalModule enters the sandbox only through externalModules, not as a value',
  })
})

test('the module object is frozen', () => {
  const id = '0F0E0D0C-0B0A-4908-8706-050403020100'
  const module = new ExternalModule(tools, { stubs: addStub, id })
  t.is(module.id, id.toLowerCase())
  t.is(module.stubs, addStub)
  t.true(Object.isFrozen(module.attrs))
  t.deepEqual(Object.keys(module.attrs), ['add', 'concat', 'VERSION'])
  t.regex(new ExternalModule({}).id, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
  t.not(new ExternalModule({}).id, new ExternalModule({}).id)
  t.throws(() => new ExternalModule({}, { id: 'nope' }), {
    instanceOf: TypeError,
    message: 'ExternalModule id must be a canonical uuid string, got "nope"',
  })
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

test('a module is not callable and names missing attributes', async () => {
  // Module errors are raised in the sandbox, as CPython's are, with no host round trip.
  await using session = await checkout(modules)
  await t.throwsAsync(session.feedRun('import tools\ntools()'), {
    instanceOf: MontyRuntimeError,
    message: "TypeError: 'module' object is not callable",
  })
  await t.throwsAsync(session.feedRun('import tools\ntools.nope()'), {
    instanceOf: MontyRuntimeError,
    message: "AttributeError: module 'tools' has no attribute 'nope'",
  })
})

test('a restored module is a module', async () => {
  // Its functions still resolve by name through the session's modules.
  let blob: Buffer
  {
    await using session = await checkout(modules)
    await session.feedRun('import tools')
    blob = await session.dump()
  }
  await using session = await checkout(modules)
  await session.loadSession(blob)
  t.is(await session.feedRun('tools.add(1, 2)'), 3)
  await t.throwsAsync(session.feedRun('tools()'), {
    instanceOf: MontyRuntimeError,
    message: "TypeError: 'module' object is not callable",
  })
  // the restored binding is still a module, carrying the id it was imported with
  t.deepEqual(await session.feedRun('[type(tools).__name__, repr(tools)]'), ['module', "<module 'tools'>"])
  const restored = (await session.feedRun('tools')) as ExternalModule
  t.true(restored instanceof ExternalModule)
  t.is(restored.id, toolsModule.id)
  await t.throwsAsync(session.feedRun('tools.nope'), {
    instanceOf: MontyRuntimeError,
    message: "AttributeError: module 'tools' has no attribute 'nope'",
  })
})

test('a ClassInstance in a module', async () => {
  class Tools {
    add(a: number, b: number): number {
      return a + b
    }
  }
  const inner = new ClassInstance(new Tools(), { allowedMethods: ['add'] })
  t.is(await run('import tools\ntools.inner.add(2, 3)', { tools: new ExternalModule({ inner }) }), 5)
})

test('attrs are copied at construction', async () => {
  // A module is frozen once made, so later edits to the object given never reach the sandbox.
  const attrs: Record<string, unknown> = { VERSION: 1 }
  const pkg = new ExternalModule(attrs)
  attrs.VERSION = 2
  attrs.sub = new ExternalModule({ pkg })
  await using session = await checkout({ pkg })
  t.is(await session.feedRun('import pkg\npkg.VERSION'), 1)
  await t.throwsAsync(session.feedRun('import pkg.sub'), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'pkg.sub'",
  })
})

test('a replaced entry is seen on the next import', async () => {
  const externalModules: ExternalModules = { tools: new ExternalModule({ VERSION: 1 }) }
  await using session = await checkout(externalModules)
  t.is(await session.feedRun('import tools\ntools.VERSION'), 1)
  externalModules.tools = new ExternalModule({ VERSION: 2 })
  t.is(await session.feedRun('import tools\ntools.VERSION'), 2)
})

test('entries added after the checkout are importable', async () => {
  // The record is read live, so a host discovering a module mid-session can offer it.
  const externalModules: ExternalModules = { tools: new ExternalModule(tools) }
  await using session = await checkout(externalModules)
  await t.throwsAsync(session.feedRun('import other'), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'other'",
  })
  externalModules.other = new ExternalModule({ VERSION: 1 })
  t.is(await session.feedRun('import other\nother.VERSION'), 1)
})

test('externalModules must be a plain object', async () => {
  await t.throwsAsync(pool().checkout({ externalModules: new Map() as never }), {
    instanceOf: TypeError,
    message: 'externalModules must be a plain object of module name to ExternalModule, got a Map',
  })
})

test('a getter is read once, at construction', async () => {
  let reads = 0
  const counted = {
    get add(): (a: number, b: number) => number {
      reads += 1
      return (a, b) => a + b
    },
  }
  const module = new ExternalModule(counted)
  t.is(reads, 1)
  await using session = await checkout({ counted: module })
  t.is(await session.feedRun('import counted\nimport counted\ncounted.add(1, 2)'), 3)
  t.is(reads, 1)
  const broken = {
    get boom(): number {
      throw new Error('nope')
    },
  }
  t.throws(() => new ExternalModule(broken), { instanceOf: Error, message: 'nope' })
})

test('name-based calls respect module exposure', async () => {
  const privateProbe = () => 'hidden'
  Object.defineProperty(privateProbe, 'name', { value: 'tools._secret' })
  await using session = await checkout({ tools: new ExternalModule({ _secret: () => 'hidden' }) })
  await t.throwsAsync(session.feedRun('import tools\nprobe()', { inputs: { probe: privateProbe } }), {
    instanceOf: MontyRuntimeError,
    message: "NameError: name 'tools._secret' is not defined",
  })
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
  await using session = await checkout({ tools: new ExternalModule(tools, { stubs }) }, { typeCheck: true })
  let step = await session.feedStart(code)
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
  await using session = await checkout(externalModules, { typeCheck: true, typeCheckFormat: 'concise' })
  await t.throwsAsync(session.feedRun("from tools import add\nadd('x', 2)"), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
  t.is(await session.feedRun('import tools\ntools.add(1, 2)'), 3)
  // Committed imports remain available to subsequent type checks.
  t.is(await session.feedRun('tools.add(3, 4)'), 7)
})

test('module stubs are fetched lazily', async () => {
  // The stubs ask runs the factory before type checking; the session reuses its result.
  let calls = 0
  const factory = () => {
    calls += 1
    return new ExternalModule(tools, { stubs: addStub })
  }
  await using session = await checkout({ tools: factory }, { typeCheck: true, typeCheckFormat: 'concise' })
  t.is(await session.feedRun('1'), 1)
  t.is(calls, 0)
  await t.throwsAsync(session.feedRun("import tools\ntools.add('x', 2)"), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
  t.is(calls, 1)
  t.is(await session.feedRun('import tools\ntools.add(1, 2)'), 3)
  t.is(calls, 1)
})

test('a module named __proto__ keeps its stub', async () => {
  const externalModules: ExternalModules = { ['__proto__']: new ExternalModule(tools, { stubs: addStub }) }
  await using session = await checkout(externalModules, { typeCheck: true, typeCheckFormat: 'concise' })
  await t.throwsAsync(session.feedRun("import __proto__\n__proto__.add('x', 2)"), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:15: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
})

test('the os handler never sees a module ask', async () => {
  const os = (name: string) => {
    throw new Error(`unexpected OS call ${name}`)
  }
  await using session = await checkout({ tools: new ExternalModule(tools, { stubs: addStub }) }, { typeCheck: true })
  t.is(await session.feedRun('import tools\ntools.add(1, 2)', { os }), 3)
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
  await using session = await checkout(externalModules, { typeCheck: true })
  t.is(await session.feedRun('import tools, other\ntools.add(other.VERSION, 1)'), 3)
})

test('a module without stubs does not type-check', async () => {
  const unresolved = {
    instanceOf: MontyTypingError,
    message: 'TypeError: main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`',
  }
  let blob: Buffer
  {
    await using session = await checkout(modules, { typeCheck: true, typeCheckFormat: 'concise' })
    await t.throwsAsync(session.feedRun('import tools'), unresolved)
    blob = await session.dump()
  }
  // The answer is final for the session and travels in its dump: a stub supplied later is not asked for.
  await using session = await checkout({ tools: new ExternalModule(tools, { stubs: addStub }) })
  await session.loadSession(blob)
  await t.throwsAsync(session.feedRun('import tools'), unresolved)
})

test('module stubs ride in a dump', async () => {
  let blob: Buffer
  {
    const externalModules: ExternalModules = { tools: new ExternalModule(tools, { stubs: addStub }) }
    await using session = await checkout(externalModules, { typeCheck: true, typeCheckFormat: 'concise' })
    t.is(await session.feedRun('import tools\ntools.add(1, 2)'), 3)
    blob = await session.dump()
  }
  // Dumps retain type checking, stubs and imports; the restoring checkout supplies the module values.
  await using session = await checkout(modules)
  await session.loadSession(blob)
  await t.throwsAsync(session.feedRun("tools.add('x', 2)"), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:1:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
  t.is(await session.feedRun('tools.add(3, 4)'), 7)
})

test('an invalid module name is refused when a call reaches it', async () => {
  // No import can name such an entry; a forged dotted call is the only way to reach it.
  const cases: [string, string][] = [
    ['for', 'TypeError: externalModules.for is not a valid module name'],
    ['1tools', 'TypeError: externalModules.1tools is not a valid module name'],
    ['json', 'TypeError: externalModules.json is provided by the sandbox and cannot be replaced'],
  ]
  for (const [name, message] of cases) {
    const probe = () => 'hidden'
    Object.defineProperty(probe, 'name', { value: `${name}.f` })
    await using session = await checkout({ [name]: new ExternalModule({}) })
    await t.throwsAsync(session.feedRun('probe()', { inputs: { probe } }), { instanceOf: MontyRuntimeError, message })
  }
})

test('a sandbox module name is never looked up', async () => {
  // the host's stub is never requested either: the checker knows `json` itself
  await using typed = await checkout(
    { json: new ExternalModule({ VERSION: 99 }, { stubs: 'VERSION: int\n' }) },
    {
      typeCheck: true,
      typeCheckFormat: 'concise',
    },
  )
  await t.throwsAsync(typed.feedRun('import json\njson.VERSION'), {
    instanceOf: MontyTypingError,
    message: 'TypeError: main.py:2:1: error[unresolved-attribute] Module `json` has no member `VERSION`',
  })
  // Compare the binding's independent list against the sandbox's actual modules.
  await using plain = await pool().checkout()
  const names = (await plain.feedRun('import sys\nlist(sys.builtin_module_names)')) as string[]
  t.true(names.includes('json'))
  const externalModules: ExternalModules = {}
  for (const name of names) {
    externalModules[name] = new ExternalModule({ VERSION: 99 }, { stubs: 'VERSION: int\n' })
  }
  await using session = await checkout(externalModules)
  for (const name of names) {
    t.is(await session.feedRun(`import ${name}\nhasattr(${name}, 'VERSION')`), false)
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
  await using session = await checkout({ pkg })
  const code = [
    'import pkg.sub',
    'import pkg.sub as s',
    'from pkg import sub',
    'from pkg.sub import add, VERSION',
    '[pkg.VERSION, pkg.sub.VERSION, s.add(1, 2), sub.add(3, 4), add(5, 6), VERSION, type(pkg.sub).__name__]',
  ].join('\n')
  t.deepEqual(await session.feedRun(code), [1, 2, 3, 7, 11, 2, 'module'])
  await t.throwsAsync(session.feedRun('import pkg.nope'), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'pkg.nope'",
  })
  await t.throwsAsync(session.feedRun('from pkg.sub.deeper import x'), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'pkg.sub.deeper'",
  })
  await t.throwsAsync(session.feedRun('import pkg.sub\npkg.sub()'), {
    instanceOf: MontyRuntimeError,
    message: "TypeError: 'module' object is not callable",
  })
  // an attribute that is not a module is not a submodule
  await t.throwsAsync(session.feedRun('import pkg.VERSION'), {
    instanceOf: MontyRuntimeError,
    message: "ModuleNotFoundError: No module named 'pkg.VERSION'",
  })
})

test('a ClassInstance in a submodule', async () => {
  class Tools {
    add(a: number, b: number): number {
      return a + b
    }
  }
  const sub = new ExternalModule({ tools: new ClassInstance(new Tools(), { allowedMethods: ['add'] }) })
  t.is(await run('from pkg.sub import tools\ntools.add(2, 3)', { pkg: new ExternalModule({ sub }) }), 5)
})

test('submodule stubs type-check as a package', async () => {
  const sub = new ExternalModule({ add: tools.add }, { stubs: addStub })
  const pkg = new ExternalModule({ VERSION: 1, sub }, { stubs: 'VERSION: int\n' })
  await using session = await checkout({ pkg }, { typeCheck: true, typeCheckFormat: 'concise' })
  const code = 'import pkg.sub\nfrom pkg import VERSION\npkg.sub.add(1, VERSION)'
  t.is(await session.feedRun(code), 2)
  await t.throwsAsync(session.feedRun("from pkg.sub import add\nadd('x', 2)"), {
    instanceOf: MontyTypingError,
    message:
      'TypeError: main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`',
  })
})

test('invalid modules are rejected at construction', () => {
  // Attrs are checked once, when the module is made: a plain object with identifier
  // keys, submodules under valid module names, and arbitrary objects refused since
  // they could expose imports.
  const rejected = (make: () => unknown, message: string) => t.throws(make, { instanceOf: TypeError, message })
  rejected(
    () => new ExternalModule((() => tools) as never),
    'ExternalModule attrs must be a plain object, got function',
  )
  rejected(
    () => new ExternalModule({ '1sub': new ExternalModule(tools) }),
    "ExternalModule attrs key '1sub' is not a valid identifier",
  )
  // each key must be one component of a host function's dotted path
  rejected(
    () => new ExternalModule({ 'a.b': (a: number, b: number) => a + b }),
    "ExternalModule attrs key 'a.b' is not a valid identifier",
  )
  rejected(() => new ExternalModule(tools, { stubs: 1 as never }), 'ExternalModule stubs must be a string, got number')
  rejected(() => new ExternalModule({ for: tools.add }), "ExternalModule attrs key 'for' is not a valid identifier")
  rejected(
    () => new ExternalModule({ json: new ExternalModule(tools) }),
    'ExternalModule attrs.json is provided by the sandbox and cannot be replaced',
  )
  // a submodule behind a getter is made, and checked, when the getter is read
  const lazy = {
    get sub(): ExternalModule {
      return new ExternalModule({ '1x': 1 })
    },
  }
  rejected(() => new ExternalModule(lazy), "ExternalModule attrs key '1x' is not a valid identifier")
})

test('an entry must be an ExternalModule or a factory', async () => {
  await t.throwsAsync(run('import tools', { tools: tools as never }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.tools must be an ExternalModule or a function returning one',
  })
  // an own key holding `undefined` is a bad entry, not a missing module
  await t.throwsAsync(run('import tools', { tools: undefined as never }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.tools must be an ExternalModule or a function returning one',
  })
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

test('every host module is a module', async () => {
  const code = 'import a\nimport b\n[type(a).__name__, type(b).__name__, type(a) is type(b)]'
  t.deepEqual(await run(code, { a: new ExternalModule({ x: 1 }), b: new ExternalModule({ y: 2 }) }), [
    'module',
    'module',
    true,
  ])
})

test('a forged name with many dots is walked through the configured modules', async () => {
  // Stop at the first missing component, even with thousands remaining.
  const probe = () => 'hidden'
  const name = `${'a.'.repeat(5000)}f`
  Object.defineProperty(probe, 'name', { value: name })
  await using session = await checkout({
    tools: new ExternalModule(tools),
    a: new ExternalModule({ a: new ExternalModule(tools) }),
  })
  await t.throwsAsync(session.feedRun('probe()', { inputs: { probe } }), {
    instanceOf: MontyRuntimeError,
    message: `NameError: name '${name}' is not defined`,
  })
  t.is(await session.feedRun('from a.a import add\nadd(1, 2)'), 3)
})

test('a module factory runs once per session, when first needed', async () => {
  let calls = 0
  const factory = () => {
    calls += 1
    return new ExternalModule(tools)
  }
  await using session = await checkout({ tools: factory })
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
    await using session = await checkout({ tools: first })
    await session.feedRun('import tools')
    blob = await session.dump()
  }
  const factory = async (): Promise<ExternalModule> => {
    await new Promise((resolve) => setTimeout(resolve, 1))
    return new ExternalModule({ add: async (a: number, b: number) => a + b, sub: (a: number, b: number) => a - b })
  }
  await using session = await checkout({ tools: factory })
  await session.loadSession(blob)
  const code = '[await tools.add(1, 2), tools.sub(5, 3), await tools.add(3, 4)]'
  t.deepEqual(await session.feedRun(code), [3, 2, 7])
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
  await using session = await checkout({ boom, notAModule, nullModule, badValue })
  await t.throwsAsync(session.feedRun('import boom'), {
    instanceOf: MontyRuntimeError,
    message: 'RuntimeError: no tools today',
  })
  await t.throwsAsync(session.feedRun('import notAModule'), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.notAModule() returned number, not an ExternalModule',
  })
  await t.throwsAsync(session.feedRun('import nullModule'), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: externalModules.nullModule() returned null, not an ExternalModule',
  })
  await t.throwsAsync(session.feedRun('import badValue'), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: ExternalModule attrs must be a plain object, got number',
  })
  t.is(await session.feedRun('1 + 1'), 2)
})

test('attrs must be a plain object', async () => {
  // Class instances require an explicit exposure policy through ClassInstance.
  class Tools {
    add(a: number, b: number): number {
      return a + b
    }
  }
  const asModule = (value: unknown) => new ExternalModule(value as Record<string, unknown>)
  t.throws(() => asModule(new Tools()), {
    instanceOf: TypeError,
    message: 'ExternalModule attrs must be a plain object, got a Tools',
  })
  t.throws(() => asModule(new Map()), {
    instanceOf: TypeError,
    message: 'ExternalModule attrs must be a plain object, got a Map',
  })
  await t.throwsAsync(run('import tools', { tools: () => asModule(new Tools()) }), {
    instanceOf: MontyRuntimeError,
    message: 'TypeError: ExternalModule attrs must be a plain object, got a Tools',
  })
  // Module namespaces have null prototypes.
  const bare = Object.assign(Object.create(null) as Record<string, unknown>, tools)
  t.deepEqual(await run(code, { tools: new ExternalModule(bare) }), [3, 'ab', 3])
})
