// Imports of `.py` files, shared by the native and wasm backends: an import
// `externalModules` does not answer searches its candidate files through the
// `os` handler first and the feed's mounts second.
import { test } from 'vitest'
import { t } from './assertions.js'
import { skipIfWasm } from './env.js'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { ExternalModule, MontyRuntimeError, NOT_HANDLED } from '@pydantic/monty'
import { MountDir } from '@pydantic/monty/node'
import { setupPool } from './helpers.js'

const { run } = setupPool()

/** A project directory mounted at `/proj`, removed once `body` finishes. */
async function withProject(body: (mount: MountDir) => Promise<void>): Promise<void> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'monty-imports-'))
  fs.writeFileSync(path.join(dir, 'helpers.py'), 'def double(x):\n    return x * 2\n')
  fs.mkdirSync(path.join(dir, 'pkg'))
  fs.writeFileSync(path.join(dir, 'pkg', '__init__.py'), '')
  fs.writeFileSync(path.join(dir, 'pkg', 'sub.py'), 'value = 3\n')
  const mount = new MountDir({ hostPath: dir, virtualPath: '/proj', mode: 'read-only' })
  try {
    await body(mount)
  } finally {
    // closed before removal: Windows refuses to delete a directory a mount holds
    mount.close()
    fs.rmSync(dir, { recursive: true, force: true })
  }
}

test('the os handler serves module files', async () => {
  const reads: unknown[] = []
  const handler = (name: string, args: unknown[]) => {
    reads.push([name, ...args])
    if (args[0] === '/served.py') {
      return 'answer = 42\n'
    }
    throw new MontyRuntimeError('FileNotFoundError', `No such file: ${String(args[0])}`)
  }
  t.is(await run('import served\nserved.answer', { os: handler }), 42)
  t.deepEqual(reads, [
    ['Path.read_text', '/served/__init__.py'],
    ['Path.read_text', '/served.py'],
  ])
})

test('the os handler sees normalized paths', async () => {
  const reads: unknown[] = []
  const handler = (_name: string, args: unknown[]) => {
    reads.push(args[0])
    return args[0] === '/pkg/__init__.py' ? "__path__ = ['/allowed/../private']\n" : NOT_HANDLED
  }
  const error = await t.throwsAsync(() => run('import pkg.secrets', { os: handler }))
  t.is(error.message, "ModuleNotFoundError: No module named 'pkg.secrets'")
  // a `..` in a sandbox-set `__path__` is resolved before the handler sees it
  t.deepEqual(reads, ['/pkg/__init__.py', '/private/secrets/__init__.py', '/private/secrets.py'])
})

test('an async os handler serves module files', async () => {
  const handler = async (_name: string, args: unknown[]) => (args[0] === '/served.py' ? 'answer = 42\n' : NOT_HANDLED)
  t.is(await run('import served\nserved.answer', { os: handler }), 42)
})

test('a read error raises at the import', async () => {
  const handler = () => {
    throw new MontyRuntimeError('OSError', 'disk on fire')
  }
  const error = await t.throwsAsync(() => run('import helpers', { os: handler }))
  t.is(error.message, 'OSError: disk on fire')
})

test('a missing module is not found', async () => {
  const error = await t.throwsAsync(() => run('import nope'))
  t.is(error.message, "ModuleNotFoundError: No module named 'nope'")
})

test('a mounted module imports', async (ctx) => {
  skipIfWasm(ctx)
  await withProject(async (mount) => {
    const code = 'import helpers\nfrom pkg.sub import value\n(helpers.double(value), helpers.__file__)'
    t.deepEqual(await run(code, { mount }), [6, '/proj/helpers.py'])
  })
})

test('the os handler is read before mounts', async (ctx) => {
  skipIfWasm(ctx)
  await withProject(async (mount) => {
    const handler = (_name: string, args: unknown[]) =>
      args[0] === '/proj/helpers.py' ? "where = 'os'\n" : NOT_HANDLED
    // the handler shadows the mount's `helpers.py` and declines `pkg`, which the mount serves
    const code = 'import helpers\nimport pkg.sub\n(helpers.where, pkg.sub.value)'
    t.deepEqual(await run(code, { mount, os: handler }), ['os', 3])
  })
})

test('host modules come before files', async (ctx) => {
  skipIfWasm(ctx)
  await withProject(async (mount) => {
    const externalModules = { helpers: new ExternalModule({ where: 'host' }) }
    t.is(await run('import helpers\nhelpers.where', { mount, externalModules }), 'host')
  })
})
