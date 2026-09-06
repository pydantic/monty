import { test } from 'vitest'
import { t } from './assertions.js'
import { skipIfBrowser, skipIfNode } from './env.js'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { MontyFileHandle, MontyRuntimeError, MountDir, type MountDirOptions } from '@pydantic/monty/node'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

// =============================================================================
// Helper: create a temporary directory with test files
// =============================================================================

function createTestDir(): {
  dir: string
  mount: (options: Omit<MountDirOptions, 'hostPath'>) => MountDir
  cleanup: () => void
} {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'monty-mount-test-'))
  fs.writeFileSync(path.join(dir, 'hello.txt'), 'hello world')
  fs.writeFileSync(path.join(dir, 'data.bin'), Buffer.from([0x00, 0x01, 0x02]))
  fs.mkdirSync(path.join(dir, 'subdir'))
  fs.writeFileSync(path.join(dir, 'subdir', 'nested.txt'), 'nested content')
  const mounts: MountDir[] = []
  return {
    dir,
    // Mounts made here are closed by `cleanup`: Windows will not remove a
    // directory while a mount still holds it open.
    mount: (options) => {
      const md = new MountDir({ hostPath: dir, ...options })
      mounts.push(md)
      return md
    },
    cleanup: () => {
      for (const md of mounts) {
        md.close()
      }
      fs.rmSync(dir, { recursive: true, force: true })
    },
  }
}

// =============================================================================
// MountDir validation
// =============================================================================

test('browser wasm reports mounts as unsupported', async (ctx) => {
  skipIfNode(ctx)
  await using session = await pool().checkout()

  const error = await t.throwsAsync(() =>
    session.feedRun("open('/mnt/data/file.txt').read()", { mount: [{}] as never }),
  )
  t.is(error.message, 'the wasm worker does not support filesystem mounts (browser has no host filesystem)')
})

test('a mount follows its directory across feeds, not its path', async (ctx) => {
  skipIfBrowser(ctx)
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'monty-mount-pin-'))
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), 'monty-mount-out-'))
  // Closed before the directories are removed; Windows would refuse otherwise.
  let child: MountDir | undefined
  let parent: MountDir | undefined
  try {
    const shared = path.join(base, 'shared')
    fs.mkdirSync(shared)
    fs.writeFileSync(path.join(shared, 'inside.txt'), 'in-mount')
    fs.writeFileSync(path.join(outside, 'secret.txt'), 'HOST SECRET')
    // Host-planted link out of the tree; the sandbox only moves it. Windows
    // gates symlink creation behind a privilege, so skip rather than fail.
    try {
      fs.symlinkSync(outside, path.join(base, 'prepared-link'), 'dir')
    } catch {
      ctx.skip()
    }

    child = new MountDir({ hostPath: shared, virtualPath: '/child', mode: 'read-only' })
    parent = new MountDir({ hostPath: base, virtualPath: '/parent', mode: 'read-write' })
    await using session = await pool().checkout()

    // Feed 1: swap the real directory out and the link in, under one name.
    // Windows refuses to rename a directory while a handle to it is open, and
    // the child mount holds one, so the swap cannot be staged there at all.
    try {
      await session.feedRun(
        `from pathlib import Path
Path('/parent/shared').rename('/parent/old-shared')
Path('/parent/prepared-link').rename('/parent/shared')`,
        { mount: parent },
      )
    } catch (err) {
      if (process.platform === 'win32') {
        ctx.skip()
      }
      throw err
    }

    // Feed 2: the same mount object, and the redirect has not reached it.
    const result = await session.feedRun(
      `from pathlib import Path
f"{Path('/child/inside.txt').read_text()}:{Path('/child/secret.txt').exists()}"`,
      { mount: child },
    )
    t.is(result, 'in-mount:False')
  } finally {
    child?.close()
    parent?.close()
    fs.rmSync(base, { recursive: true, force: true })
    fs.rmSync(outside, { recursive: true, force: true })
  }
})

// =============================================================================
// Working directory
// =============================================================================

test('cwd defaults to the root without mounts', async () => {
  t.deepEqual(await run('import os\n(os.getcwd(), __file__)'), ['/', '/main.py'])
  t.deepEqual(await run('import os\nos.getcwd()', { cwd: '/work/' }), '/work')
})

test('cwd defaults to the first mount and chdir lasts one feed', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    await using session = await pool().checkout()
    t.deepEqual(await session.feedRun("import os\n(os.getcwd(), __file__, open('hello.txt').read())", { mount: md }), [
      '/data',
      '/data/main.py',
      'hello world',
    ])
    t.is(await session.feedRun("os.chdir('subdir')\nos.getcwd()", { mount: md }), '/data/subdir')
    t.is(await session.feedRun('os.getcwd()', { mount: md }), '/data')
    t.is(await session.feedRun("open('nested.txt').read()", { mount: md, cwd: '/data/subdir' }), 'nested content')
  } finally {
    cleanup()
  }
})

test('a relative cwd is refused', async () => {
  const error = await t.throwsAsync(() => run('1', { cwd: 'data' }), { instanceOf: MontyRuntimeError })
  t.is(error.message, 'ValueError: cwd must be an absolute POSIX path: "data"')
})

test('MountDir repr', (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    t.is(md.repr(), `MountDir(host_path='${dir}', virtual_path='/data', mode='read-only')`)
  } finally {
    cleanup()
  }
})

test('MountDir invalid mode', (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const error = t.throws(() => mount({ virtualPath: '/data', mode: 'invalid' as never }))
    t.is(error?.message, "invalid mount mode: 'invalid'. Expected 'read-only', 'read-write' or 'overlay'")
  } finally {
    cleanup()
  }
})

test('MountDir attributes', (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    t.is(md.virtualPath, '/data')
    t.is(md.hostPath, dir)
    t.is(md.mode, 'read-only')
    t.is(md.memoryUsageLimit, 100_000_000)

    const limited = mount({ virtualPath: '/limited', memoryUsageLimit: 1234 })
    t.is(limited.memoryUsageLimit, 1234)

    // The opened native handle is symbol-keyed, so it is not part of the
    // public surface: no string key exposes it, and nothing reachable by name
    // can close the directory out from under a feed.
    t.deepEqual(Object.keys(md).sort(), ['hostPath', 'memoryUsageLimit', 'mode', 'virtualPath', 'writeBytesLimit'])
    t.is((md as unknown as Record<string, unknown>).native, undefined)
  } finally {
    cleanup()
  }
})

test('MountDir nonexistent host path', (ctx) => {
  skipIfBrowser(ctx)
  // The constructor opens the host directory, so a bad path fails there rather
  // than at the first feed. The OS-error suffix is platform specific.
  const error = t.throws(
    () => new MountDir({ hostPath: '/nonexistent/path/that/does/not/exist', virtualPath: '/data' }),
  )
  t.true(error.message.startsWith("TypeError: cannot open host path '/nonexistent/path/that/does/not/exist':"))
})

test('MountDir non-absolute virtual path', (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const error = t.throws(() => mount({ virtualPath: 'relative' }))
    t.is(error.message, "TypeError: virtual path must be absolute, got: 'relative'")
  } finally {
    cleanup()
  }
})

test('closing a mount releases it and later feeds are refused', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    t.is(await run("open('/data/hello.txt').read()", { mount: md }), 'hello world')

    md.close()
    md.close() // idempotent
    await t.throwsAsync(() => run("open('/data/hello.txt').read()", { mount: md }))
    // The configuration it reports outlives the descriptor.
    t.is(md.virtualPath, '/data')

    // `using` closes it the same way at the end of the block.
    let disposed: MountDir
    {
      using scoped = mount({ virtualPath: '/scoped', mode: 'read-only' })
      t.is(await run("open('/scoped/hello.txt').read()", { mount: scoped }), 'hello world')
      disposed = scoped
    }
    await t.throwsAsync(() => run("open('/scoped/hello.txt').read()", { mount: disposed }))
  } finally {
    cleanup()
  }
})

test('MountDir default mode is overlay', (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data' })
    t.is(md.mode, 'overlay')
  } finally {
    cleanup()
  }
})

test('MountDir write_bytes_limit', (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', writeBytesLimit: 1024 })
    t.is(md.writeBytesLimit, 1024)

    const md2 = mount({ virtualPath: '/data' })
    t.is(md2.writeBytesLimit, null)
  } finally {
    cleanup()
  }
})

// =============================================================================
// Callback-backed file handles
// =============================================================================

test('MontyFileHandle exposes canonical file properties', () => {
  const handle = new MontyFileHandle('/data/message.bin', 'br', { position: 12 })
  t.deepEqual(Object.keys(handle), ['path', 'mode', 'position'])
  t.is(handle.path, '/data/message.bin')
  t.is(handle.mode, 'rb')
  t.is(handle.position, 12)
  t.is(handle.binary, true)
  t.is(handle.readable, true)
  t.is(handle.writable, false)
  t.true(Object.isFrozen(handle))
})

test('os callback file handles support text and binary reads', async () => {
  const calls: [string, unknown[]][] = []
  const osCallback = (name: string, args: unknown[]) => {
    calls.push([name, args])
    const path = args[0] as string
    if (name === 'open') {
      return new MontyFileHandle(path, args[1] as string)
    } else if (name === 'Path.read_text') {
      return 'hello'
    } else if (name === 'Path.read_bytes') {
      return Uint8Array.from([0, 1, 2])
    }
    throw new Error(`unexpected OS call: ${name}`)
  }

  t.is(await run("open('/data/message.txt').read()", { os: osCallback }), 'hello')
  const binary = (await run("open('/data/data.bin', 'rb').read()", { os: osCallback })) as Uint8Array
  t.deepEqual([...binary], [0, 1, 2])
  t.deepEqual(calls, [
    ['open', ['/data/message.txt', 'r']],
    ['Path.read_text', ['/data/message.txt']],
    ['open', ['/data/data.bin', 'rb']],
    ['Path.read_bytes', ['/data/data.bin']],
  ])
})

test('file handles round-trip with canonical modes and positions', async () => {
  await using session = await pool().checkout()
  const returned = (await session.feedRun("open('/data/message.txt')", {
    os: (_name, args) => new MontyFileHandle(args[0] as string, 'tr', { position: 42 }),
  })) as MontyFileHandle
  t.true(returned instanceof MontyFileHandle)
  t.deepEqual(returned, new MontyFileHandle('/data/message.txt', 'r', { position: 42 }))
  t.is(returned.binary, false)
  t.is(returned.readable, true)
  t.is(returned.writable, false)

  const returnedAgain = await session.feedRun("open('/data/message.txt')", { os: () => returned })
  t.deepEqual(returnedAgain, returned)

  const defaultPosition = await session.feedRun("open('/data/default.txt')", {
    os: (_name, args) => new MontyFileHandle(args[0] as string, 'r'),
  })
  t.deepEqual(defaultPosition, new MontyFileHandle('/data/default.txt', 'r'))
})

test('MontyFileHandle rejects invalid arguments', () => {
  const cases: [() => unknown, string][] = [
    [() => new MontyFileHandle(1 as unknown as string, 'r'), 'MontyFileHandle path must be a string'],
    [() => new MontyFileHandle('/x', 1 as unknown as string), 'MontyFileHandle mode must be a string'],
    [() => new MontyFileHandle('/x', 'q'), "invalid mode: 'q'"],
    [
      () => new MontyFileHandle('/x', 'b'),
      'Must have exactly one of create/read/write/append mode and at most one plus',
    ],
  ]
  for (const position of [-1, 1.5, Number.POSITIVE_INFINITY, Number.MAX_SAFE_INTEGER + 1, '0']) {
    cases.push([
      () => new MontyFileHandle('/x', 'r', { position: position as number }),
      'MontyFileHandle position must be a non-negative safe integer',
    ])
  }

  for (const [create, expected] of cases) {
    t.throws(create, { instanceOf: TypeError, message: expected })
  }
})

// =============================================================================
// Read operations (read-only mount)
// =============================================================================

test('read_text via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const result = await run("from pathlib import Path; Path('/data/hello.txt').read_text()", { mount: md })
    t.is(result, 'hello world')
  } finally {
    cleanup()
  }
})

test('read_bytes via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const result = await run("from pathlib import Path; Path('/data/data.bin').read_bytes()", { mount: md })
    t.deepEqual(result, Buffer.from([0x00, 0x01, 0x02]))
  } finally {
    cleanup()
  }
})

test('path exists via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const code = `
from pathlib import Path
exists_file = Path('/data/hello.txt').exists()
exists_dir = Path('/data/subdir').exists()
exists_missing = Path('/data/nope.txt').exists()
[exists_file, exists_dir, exists_missing]
`
    t.deepEqual(await run(code, { mount: md }), [true, true, false])
  } finally {
    cleanup()
  }
})

test('is_file and is_dir via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const code = `
from pathlib import Path
[Path('/data/hello.txt').is_file(), Path('/data/hello.txt').is_dir(),
 Path('/data/subdir').is_file(), Path('/data/subdir').is_dir()]
`
    t.deepEqual(await run(code, { mount: md }), [true, false, false, true])
  } finally {
    cleanup()
  }
})

test('iterdir via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const code = `
from pathlib import Path
sorted([p.name for p in Path('/data').iterdir()])
`
    t.deepEqual(await run(code, { mount: md }), ['data.bin', 'hello.txt', 'subdir'])
  } finally {
    cleanup()
  }
})

test('stat via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const code = `
from pathlib import Path
s = Path('/data/hello.txt').stat()
s.st_size
`
    t.is(await run(code, { mount: md }), 11)
  } finally {
    cleanup()
  }
})

test('read nested file via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const result = await run("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()", { mount: md })
    t.is(result, 'nested content')
  } finally {
    cleanup()
  }
})

// =============================================================================
// Write operations
// =============================================================================

test('write blocked on read-only mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const error = await t.throwsAsync(
      () => run("from pathlib import Path; Path('/data/new.txt').write_text('x')", { mount: md }),
      { instanceOf: MontyRuntimeError },
    )
    t.is(error.message, "PermissionError: [Errno 30] Read-only file system: '/data/new.txt'")
  } finally {
    cleanup()
  }
})

test('write succeeds on read-write mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-write' })
    const code = `
from pathlib import Path
Path('/data/new.txt').write_text('written by monty')
Path('/data/new.txt').read_text()
`
    t.is(await run(code, { mount: md }), 'written by monty')
    // Verify it was actually written to the host filesystem
    t.is(fs.readFileSync(path.join(dir, 'new.txt'), 'utf-8'), 'written by monty')
  } finally {
    cleanup()
  }
})

test('overlay write does not modify host', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/overlay_file.txt').write_text('overlay content')
Path('/data/overlay_file.txt').read_text()
`
    t.is(await run(code, { mount: md }), 'overlay content')
    // Verify host filesystem was NOT modified
    t.false(fs.existsSync(path.join(dir, 'overlay_file.txt')))
  } finally {
    cleanup()
  }
})

test('overlay read falls through to host', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    const result = await run("from pathlib import Path; Path('/data/hello.txt').read_text()", { mount: md })
    t.is(result, 'hello world')
  } finally {
    cleanup()
  }
})

test('overlay writes do not persist across runs', async (ctx) => {
  skipIfBrowser(ctx)
  // Overlay state lives in the pool's per-feed mount table, so unlike the old
  // in-process API it does NOT persist across runs sharing the same MountDir.
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    await run("from pathlib import Path; Path('/data/persistent.txt').write_text('run1')", { mount: md })
    const error = await t.throwsAsync(
      () => run("from pathlib import Path; Path('/data/persistent.txt').read_text()", { mount: md }),
      { instanceOf: MontyRuntimeError },
    )
    t.is(error.message, "FileNotFoundError: [Errno 2] No such file or directory: '/data/persistent.txt'")
  } finally {
    cleanup()
  }
})

test('overlay memory usage limit is aggregate', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay', memoryUsageLimit: 1000 })
    const code = `
from pathlib import Path
p = Path('/data/retained.bin')
p.write_bytes(b'a' * 500)
p.read_bytes()
`
    const error = await t.throwsAsync(() => run(code, { mount: md }), { instanceOf: MontyRuntimeError })
    t.is(error.message, 'MemoryError: mount memory usage limit of 1 KB exceeded')
  } finally {
    cleanup()
  }
})

// =============================================================================
// Path operations
// =============================================================================

test('mkdir and rmdir via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/newdir').mkdir()
exists = Path('/data/newdir').is_dir()
Path('/data/newdir').rmdir()
after = Path('/data/newdir').exists()
[exists, after]
`
    t.deepEqual(await run(code, { mount: md }), [true, false])
  } finally {
    cleanup()
  }
})

test('unlink via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/hello.txt').unlink()
Path('/data/hello.txt').exists()
`
    t.is(await run(code, { mount: md }), false)
    // Host file should still exist (overlay mode)
    t.true(fs.existsSync(path.join(dir, 'hello.txt')))
  } finally {
    cleanup()
  }
})

test('rename via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/hello.txt').rename('/data/renamed.txt')
[Path('/data/hello.txt').exists(), Path('/data/renamed.txt').read_text()]
`
    t.deepEqual(await run(code, { mount: md }), [false, 'hello world'])
  } finally {
    cleanup()
  }
})

test('resolve via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const result = await run("from pathlib import Path; str(Path('/data/subdir/../hello.txt').resolve())", {
      mount: md,
    })
    t.is(result, '/data/hello.txt')
  } finally {
    cleanup()
  }
})

// =============================================================================
// Security
// =============================================================================

test('path traversal blocked', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const error = await t.throwsAsync(
      () => run("from pathlib import Path; Path('/data/../../etc/passwd').read_text()", { mount: md }),
      { instanceOf: MontyRuntimeError },
    )
    t.is(error.message, "PermissionError: Permission denied: '/data/../../etc/passwd'")
  } finally {
    cleanup()
  }
})

test('unmounted path denied', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const error = await t.throwsAsync(
      () => run("from pathlib import Path; Path('/other/file.txt').exists()", { mount: md }),
      { instanceOf: MontyRuntimeError },
    )
    t.is(error.message, "PermissionError: Permission denied: '/other/file.txt'")
  } finally {
    cleanup()
  }
})

// =============================================================================
// Non-filesystem ops (no `os` callback - returns error)
// =============================================================================

test('non-filesystem os call without fallback', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const error = await t.throwsAsync(() => run("import os; os.getenv('PATH')", { mount: md }), {
      instanceOf: MontyRuntimeError,
    })
    t.is(error.message, "RuntimeError: 'os.getenv' is not supported in this environment")
  } finally {
    cleanup()
  }
})

// =============================================================================
// Multiple mounts
// =============================================================================

test('multiple mounts with different modes', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup: cleanup1 } = createTestDir()
  const dir2 = fs.mkdtempSync(path.join(os.tmpdir(), 'monty-mount-test2-'))
  fs.writeFileSync(path.join(dir2, 'file2.txt'), 'from mount2')
  // Closed in `finally`: Windows will not remove dir2 while this mount holds
  // it open. The `/ro` one is closed by `cleanup1`.
  const writable = new MountDir({ hostPath: dir2, virtualPath: '/rw', mode: 'read-write' })
  try {
    const mounts = [mount({ virtualPath: '/ro', mode: 'read-only' }), writable]
    const code = `
from pathlib import Path
a = Path('/ro/hello.txt').read_text()
b = Path('/rw/file2.txt').read_text()
[a, b]
`
    t.deepEqual(await run(code, { mount: mounts }), ['hello world', 'from mount2'])
  } finally {
    writable.close()
    cleanup1()
    fs.rmSync(dir2, { recursive: true, force: true })
  }
})

// =============================================================================
// Mount with external functions
// =============================================================================

test('mount works with external functions', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const code = `
from pathlib import Path
content = Path('/data/hello.txt').read_text()
result = get_prefix()
result + content
`
    const result = await run(code, { mount: md, externalLookup: { get_prefix: () => 'PREFIX: ' } })
    t.is(result, 'PREFIX: hello world')
  } finally {
    cleanup()
  }
})

// =============================================================================
// Session (REPL) mount support
// =============================================================================

test('session feed with mount read', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    await session.feedRun('from pathlib import Path', { mount: md })
    t.is(await session.feedRun("Path('/data/hello.txt').read_text()", { mount: md }), 'hello world')
  } finally {
    await session.close()
    cleanup()
  }
})

// The mount table is rebuilt per feed on the host side of the pool (see
// limitations/pool-architecture.md): overlay writes live for the duration of
// one feed and are discarded when it ends, unlike the old in-process API
// where overlay state persisted on the MountDir object.

test('session overlay write is discarded between feeds', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    await session.feedRun('from pathlib import Path', { mount: md })
    await session.feedRun("Path('/data/new.txt').write_text('from repl')", { mount: md })
    const error = await t.throwsAsync(() => session.feedRun("Path('/data/new.txt').read_text()", { mount: md }), {
      instanceOf: MontyRuntimeError,
    })
    t.is(error.message, "FileNotFoundError: [Errno 2] No such file or directory: '/data/new.txt'")
    // Host not modified either
    t.false(fs.existsSync(path.join(dir, 'new.txt')))
  } finally {
    await session.close()
    cleanup()
  }
})

test('session overlay overwrite reverts between feeds', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    await session.feedRun('from pathlib import Path', { mount: md })
    await session.feedRun("Path('/data/hello.txt').write_text('version1')", { mount: md })
    // The next feed sees the pristine host content again ...
    t.is(await session.feedRun("Path('/data/hello.txt').read_text()", { mount: md }), 'hello world')
    // ... and the host file was never touched.
    t.is(fs.readFileSync(path.join(dir, 'hello.txt'), 'utf-8'), 'hello world')
  } finally {
    await session.close()
    cleanup()
  }
})

test('session overlay delete reverts between feeds', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    await session.feedRun('from pathlib import Path', { mount: md })
    await session.feedRun("Path('/data/hello.txt').unlink()", { mount: md })
    // The deletion only lived inside the previous feed's overlay.
    t.is(await session.feedRun("Path('/data/hello.txt').exists()", { mount: md }), true)
    t.true(fs.existsSync(path.join(dir, 'hello.txt')))
  } finally {
    await session.close()
    cleanup()
  }
})

test('overlay mkdir and nested write within one feed', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/mydir').mkdir()
Path('/data/mydir/file.txt').write_text('nested')
Path('/data/mydir/file.txt').read_text()
`
    t.is(await run(code, { mount: md }), 'nested')
    t.false(fs.existsSync(path.join(dir, 'mydir')))
  } finally {
    cleanup()
  }
})

test('overlay iterdir sees overlay files', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  try {
    const md = mount({ virtualPath: '/data', mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/extra.txt').write_text('extra')
sorted([p.name for p in Path('/data').iterdir()])
`
    t.deepEqual(await run(code, { mount: md }), ['data.bin', 'extra.txt', 'hello.txt', 'subdir'])
  } finally {
    cleanup()
  }
})

test('session read-write mount writes to host', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-write' })
    await session.feedRun('from pathlib import Path', { mount: md })
    await session.feedRun("Path('/data/rw_file.txt').write_text('written')", { mount: md })
    t.is(await session.feedRun("Path('/data/rw_file.txt').read_text()", { mount: md }), 'written')
    // Host was actually modified
    t.is(fs.readFileSync(path.join(dir, 'rw_file.txt'), 'utf-8'), 'written')
  } finally {
    await session.close()
    cleanup()
  }
})

test('session read-only mount blocks write', async (ctx) => {
  skipIfBrowser(ctx)
  const { mount, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    await session.feedRun('from pathlib import Path', { mount: md })
    const error = await t.throwsAsync(() => session.feedRun("Path('/data/nope.txt').write_text('x')", { mount: md }), {
      instanceOf: MontyRuntimeError,
    })
    t.is(error.message, "PermissionError: [Errno 30] Read-only file system: '/data/nope.txt'")
  } finally {
    await session.close()
    cleanup()
  }
})

// =============================================================================
// Symlinks inside a mount
// =============================================================================

/** Create a symlink, returning false where the host forbids it (Windows
 *  without Developer Mode or elevation), so those tests skip rather than fail. */
function makeSymlink(target: string, link: string): boolean {
  try {
    fs.symlinkSync(target, link)
    return true
  } catch {
    return false
  }
}

test('relative symlink inside a mount is followed', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  try {
    if (!makeSymlink('hello.txt', path.join(dir, 'rel_link.txt'))) return
    const md = mount({ virtualPath: '/data', mode: 'read-only' })
    const result = await run("from pathlib import Path; Path('/data/rel_link.txt').read_text()", { mount: md })
    t.is(result, 'hello world')
  } finally {
    cleanup()
  }
})

test('absolute symlink target is refused inside a mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, mount, cleanup } = createTestDir()
  try {
    // Absolute even though it points back into the same mount: a descriptor has
    // no path of its own, so a leading '/' cannot be interpreted.
    if (!makeSymlink(path.join(dir, 'hello.txt'), path.join(dir, 'abs_link.txt'))) return
    const md = mount({ virtualPath: '/data', mode: 'read-only' })

    const error = await t.throwsAsync(
      () => run("from pathlib import Path; Path('/data/abs_link.txt').read_text()", { mount: md }),
      {
        instanceOf: MontyRuntimeError,
      },
    )
    t.is(error.message, "PermissionError: [Errno 13] Permission denied: '/data/abs_link.txt'")

    // Predicates answer False rather than raising.
    const seen = await run("from pathlib import Path; p = Path('/data/abs_link.txt'); (p.exists(), p.is_file())", {
      mount: md,
    })
    t.deepEqual(seen, [false, false])
  } finally {
    cleanup()
  }
})
