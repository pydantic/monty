import { test } from 'vitest'
import { t } from './assertions.js'
import { skipIfBrowser, skipIfNode } from './env.js'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { MontyRuntimeError, MountDir } from '@pydantic/monty/node'
import { setupPool } from './helpers.js'

const { run, pool } = setupPool()

// =============================================================================
// Helper: create a temporary directory with test files
// =============================================================================

function createTestDir(): { dir: string; cleanup: () => void } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'monty-mount-test-'))
  fs.writeFileSync(path.join(dir, 'hello.txt'), 'hello world')
  fs.writeFileSync(path.join(dir, 'data.bin'), Buffer.from([0x00, 0x01, 0x02]))
  fs.mkdirSync(path.join(dir, 'subdir'))
  fs.writeFileSync(path.join(dir, 'subdir', 'nested.txt'), 'nested content')
  return {
    dir,
    cleanup: () => fs.rmSync(dir, { recursive: true, force: true }),
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

test('MountDir repr', (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
    t.is(md.repr(), `MountDir(virtual_path='/data', host_path='${dir}', mode='read-only')`)
  } finally {
    cleanup()
  }
})

test('MountDir invalid mode', (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const error = t.throws(() => new MountDir('/data', dir, { mode: 'invalid' as never }))
    t.is(error?.message, "invalid mount mode: 'invalid'. Expected 'read-only', 'read-write' or 'overlay'")
  } finally {
    cleanup()
  }
})

test('MountDir attributes', (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
    t.is(md.virtualPath, '/data')
    t.is(md.hostPath, dir)
    t.is(md.mode, 'read-only')
    t.is(md.memoryUsageLimit, 100_000_000)

    const limited = new MountDir('/limited', dir, { memoryUsageLimit: 1234 })
    t.is(limited.memoryUsageLimit, 1234)
  } finally {
    cleanup()
  }
})

test('MountDir nonexistent host path', async (ctx) => {
  skipIfBrowser(ctx)
  // Host paths are validated by the pool when the feed starts (not the
  // constructor). The OS-error suffix is platform specific.
  const md = new MountDir('/data', '/nonexistent/path/that/does/not/exist')
  const error = await t.throwsAsync(() => run('1 + 1', { mount: md }), { instanceOf: MontyRuntimeError })
  t.true(error.message.startsWith("TypeError: cannot canonicalize host path '/nonexistent/path/that/does/not/exist':"))
})

test('MountDir non-absolute virtual path', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('relative', dir)
    const error = await t.throwsAsync(() => run('1 + 1', { mount: md }), { instanceOf: MontyRuntimeError })
    t.is(error.message, "TypeError: virtual path must be absolute, got: 'relative'")
  } finally {
    cleanup()
  }
})

test('MountDir default mode is overlay', (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir)
    t.is(md.mode, 'overlay')
  } finally {
    cleanup()
  }
})

test('MountDir write_bytes_limit', (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { writeBytesLimit: 1024 })
    t.is(md.writeBytesLimit, 1024)

    const md2 = new MountDir('/data', dir)
    t.is(md2.writeBytesLimit, null)
  } finally {
    cleanup()
  }
})

// =============================================================================
// Read operations (read-only mount)
// =============================================================================

test('read_text via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
    const result = await run("from pathlib import Path; Path('/data/hello.txt').read_text()", { mount: md })
    t.is(result, 'hello world')
  } finally {
    cleanup()
  }
})

test('read_bytes via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
    const result = await run("from pathlib import Path; Path('/data/data.bin').read_bytes()", { mount: md })
    t.deepEqual(result, Buffer.from([0x00, 0x01, 0x02]))
  } finally {
    cleanup()
  }
})

test('path exists via mount', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-write' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay', memoryUsageLimit: 1000 })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir: dir1, cleanup: cleanup1 } = createTestDir()
  const dir2 = fs.mkdtempSync(path.join(os.tmpdir(), 'monty-mount-test2-'))
  fs.writeFileSync(path.join(dir2, 'file2.txt'), 'from mount2')
  try {
    const mounts = [new MountDir('/ro', dir1, { mode: 'read-only' }), new MountDir('/rw', dir2, { mode: 'read-write' })]
    const code = `
from pathlib import Path
a = Path('/ro/hello.txt').read_text()
b = Path('/rw/file2.txt').read_text()
[a, b]
`
    t.deepEqual(await run(code, { mount: mounts }), ['hello world', 'from mount2'])
  } finally {
    cleanup1()
    fs.rmSync(dir2, { recursive: true, force: true })
  }
})

// =============================================================================
// Mount with external functions
// =============================================================================

test('mount works with external functions', async (ctx) => {
  skipIfBrowser(ctx)
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
  const { dir, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDir('/data', dir, { mode: 'overlay' })
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
  const { dir, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-write' })
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
  const { dir, cleanup } = createTestDir()
  const session = await pool().checkout()
  try {
    const md = new MountDir('/data', dir, { mode: 'read-only' })
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
