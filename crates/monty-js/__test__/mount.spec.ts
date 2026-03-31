import test from 'ava'
import * as fs from 'fs'
import * as os from 'os'
import * as path from 'path'

import { Monty, MontyRepl, MountDirectory, MontyRuntimeError, MontySnapshot, MontyComplete } from '../wrapper'

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
// MountDirectory validation
// =============================================================================

test('MountDirectory repr', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const repr = md.repr()
    t.true(repr.includes('MountDirectory'))
    t.true(repr.includes('/data'))
  } finally {
    cleanup()
  }
})

test('MountDirectory invalid mode', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const error = t.throws(() => new MountDirectory('/data', dir, { mode: 'invalid' as any }))
    t.true(error?.message.includes("Invalid mode 'invalid'"))
  } finally {
    cleanup()
  }
})

test('MountDirectory attributes', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    t.is(md.virtualPath, '/data')
    t.is(md.mode, 'read-only')
  } finally {
    cleanup()
  }
})

test('MountDirectory nonexistent host path', (t) => {
  t.throws(() => new MountDirectory('/data', '/nonexistent/path/that/does/not/exist'))
})

test('MountDirectory non-absolute virtual path', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    t.throws(() => new MountDirectory('relative', dir))
  } finally {
    cleanup()
  }
})

test('MountDirectory default mode is overlay', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir)
    t.is(md.mode, 'overlay')
  } finally {
    cleanup()
  }
})

test('MountDirectory write_bytes_limit', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { writeBytesLimit: 1024 })
    t.is(md.writeBytesLimit, 1024)

    const md2 = new MountDirectory('/data', dir)
    t.is(md2.writeBytesLimit, null)
  } finally {
    cleanup()
  }
})

// =============================================================================
// Read operations (read-only mount)
// =============================================================================

test('read_text via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const result = new Monty("from pathlib import Path; Path('/data/hello.txt').read_text()").run({ mount: md })
    t.is(result, 'hello world')
  } finally {
    cleanup()
  }
})

test('read_bytes via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const result = new Monty("from pathlib import Path; Path('/data/data.bin').read_bytes()").run({ mount: md })
    t.deepEqual(result, Buffer.from([0x00, 0x01, 0x02]))
  } finally {
    cleanup()
  }
})

test('path exists via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const code = `
from pathlib import Path
exists_file = Path('/data/hello.txt').exists()
exists_dir = Path('/data/subdir').exists()
exists_missing = Path('/data/nope.txt').exists()
[exists_file, exists_dir, exists_missing]
`
    const result = new Monty(code).run({ mount: md })
    t.deepEqual(result, [true, true, false])
  } finally {
    cleanup()
  }
})

test('is_file and is_dir via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const code = `
from pathlib import Path
[Path('/data/hello.txt').is_file(), Path('/data/hello.txt').is_dir(),
 Path('/data/subdir').is_file(), Path('/data/subdir').is_dir()]
`
    const result = new Monty(code).run({ mount: md })
    t.deepEqual(result, [true, false, false, true])
  } finally {
    cleanup()
  }
})

test('iterdir via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const code = `
from pathlib import Path
sorted([p.name for p in Path('/data').iterdir()])
`
    const result = new Monty(code).run({ mount: md })
    t.deepEqual(result, ['data.bin', 'hello.txt', 'subdir'])
  } finally {
    cleanup()
  }
})

test('stat via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const code = `
from pathlib import Path
s = Path('/data/hello.txt').stat()
s.st_size
`
    const result = new Monty(code).run({ mount: md })
    t.is(result, 11)
  } finally {
    cleanup()
  }
})

test('read nested file via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const result = new Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()").run({
      mount: md,
    })
    t.is(result, 'nested content')
  } finally {
    cleanup()
  }
})

// =============================================================================
// Write operations
// =============================================================================

test('write blocked on read-only mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const error = t.throws(
      () => new Monty("from pathlib import Path; Path('/data/new.txt').write_text('x')").run({ mount: md }),
      { instanceOf: MontyRuntimeError },
    )
    t.true(error.message.includes('Read-only file system'))
  } finally {
    cleanup()
  }
})

test('write succeeds on read-write mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-write' })
    const code = `
from pathlib import Path
Path('/data/new.txt').write_text('written by monty')
Path('/data/new.txt').read_text()
`
    const result = new Monty(code).run({ mount: md })
    t.is(result, 'written by monty')
    // Verify it was actually written to the host filesystem
    t.is(fs.readFileSync(path.join(dir, 'new.txt'), 'utf-8'), 'written by monty')
  } finally {
    cleanup()
  }
})

test('overlay write does not modify host', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/overlay_file.txt').write_text('overlay content')
Path('/data/overlay_file.txt').read_text()
`
    const result = new Monty(code).run({ mount: md })
    t.is(result, 'overlay content')
    // Verify host filesystem was NOT modified
    t.false(fs.existsSync(path.join(dir, 'overlay_file.txt')))
  } finally {
    cleanup()
  }
})

test('overlay read falls through to host', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const result = new Monty("from pathlib import Path; Path('/data/hello.txt').read_text()").run({ mount: md })
    t.is(result, 'hello world')
  } finally {
    cleanup()
  }
})

test('overlay persists across runs', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    new Monty("from pathlib import Path; Path('/data/persistent.txt').write_text('run1')").run({ mount: md })
    const result = new Monty("from pathlib import Path; Path('/data/persistent.txt').read_text()").run({ mount: md })
    t.is(result, 'run1')
  } finally {
    cleanup()
  }
})

// =============================================================================
// Path operations
// =============================================================================

test('mkdir and rmdir via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/newdir').mkdir()
exists = Path('/data/newdir').is_dir()
Path('/data/newdir').rmdir()
after = Path('/data/newdir').exists()
[exists, after]
`
    const result = new Monty(code).run({ mount: md })
    t.deepEqual(result, [true, false])
  } finally {
    cleanup()
  }
})

test('unlink via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/hello.txt').unlink()
Path('/data/hello.txt').exists()
`
    const result = new Monty(code).run({ mount: md })
    t.is(result, false)
    // Host file should still exist (overlay mode)
    t.true(fs.existsSync(path.join(dir, 'hello.txt')))
  } finally {
    cleanup()
  }
})

test('rename via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const code = `
from pathlib import Path
Path('/data/hello.txt').rename('/data/renamed.txt')
[Path('/data/hello.txt').exists(), Path('/data/renamed.txt').read_text()]
`
    const result = new Monty(code).run({ mount: md })
    t.deepEqual(result, [false, 'hello world'])
  } finally {
    cleanup()
  }
})

test('resolve via mount', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const result = new Monty("from pathlib import Path; str(Path('/data/subdir/../hello.txt').resolve())").run({
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

test('path traversal blocked', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const error = t.throws(
      () => new Monty("from pathlib import Path; Path('/data/../../etc/passwd').read_text()").run({ mount: md }),
      { instanceOf: MontyRuntimeError },
    )
    t.true(error.message.includes('Permission denied'))
  } finally {
    cleanup()
  }
})

test('unmounted path denied', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const error = t.throws(
      () => new Monty("from pathlib import Path; Path('/other/file.txt').exists()").run({ mount: md }),
      { instanceOf: MontyRuntimeError },
    )
    t.true(error.message.includes('Permission denied'))
  } finally {
    cleanup()
  }
})

// =============================================================================
// Non-filesystem ops (no fallback in JS - returns error)
// =============================================================================

test('non-filesystem os call without fallback', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const error = t.throws(() => new Monty("import os; os.getenv('PATH')").run({ mount: md }), {
      instanceOf: MontyRuntimeError,
    })
    t.true(error.message.includes('is not supported in this environment'))
  } finally {
    cleanup()
  }
})

// =============================================================================
// Multiple mounts
// =============================================================================

test('multiple mounts with different modes', (t) => {
  const { dir: dir1, cleanup: cleanup1 } = createTestDir()
  const dir2 = fs.mkdtempSync(path.join(os.tmpdir(), 'monty-mount-test2-'))
  fs.writeFileSync(path.join(dir2, 'file2.txt'), 'from mount2')
  try {
    const mounts = [
      new MountDirectory('/ro', dir1, { mode: 'read-only' }),
      new MountDirectory('/rw', dir2, { mode: 'read-write' }),
    ]
    const code = `
from pathlib import Path
a = Path('/ro/hello.txt').read_text()
b = Path('/rw/file2.txt').read_text()
[a, b]
`
    const result = new Monty(code).run({ mount: mounts })
    t.deepEqual(result, ['hello world', 'from mount2'])
  } finally {
    cleanup1()
    fs.rmSync(dir2, { recursive: true, force: true })
  }
})

// =============================================================================
// Mount with start/resume
// =============================================================================

test('mount works with start/resume for external functions', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const code = `
from pathlib import Path
content = Path('/data/hello.txt').read_text()
result = get_prefix()
result + content
`
    const m = new Monty(code)
    const progress = m.start({ mount: md })

    // Should pause at get_prefix() after reading the file via mount
    t.true(progress instanceof MontySnapshot)
    const snapshot = progress as MontySnapshot
    t.is(snapshot.functionName, 'get_prefix')
    const complete = snapshot.resume({ returnValue: 'PREFIX: ' })
    t.true(complete instanceof MontyComplete)
    t.is((complete as MontyComplete).output, 'PREFIX: hello world')
  } finally {
    cleanup()
  }
})

// =============================================================================
// REPL mount support
// =============================================================================

test('REPL feed with mount read', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    const result = repl.feed("Path('/data/hello.txt').read_text()", { mount: md })
    t.is(result, 'hello world')
  } finally {
    cleanup()
  }
})

test('REPL overlay write persists across feeds', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    repl.feed("Path('/data/new.txt').write_text('from repl')", { mount: md })
    const result = repl.feed("Path('/data/new.txt').read_text()", { mount: md })
    t.is(result, 'from repl')
    // Host not modified
    t.false(fs.existsSync(path.join(dir, 'new.txt')))
  } finally {
    cleanup()
  }
})

test('REPL overlay overwrite persists', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    repl.feed("Path('/data/hello.txt').write_text('version1')", { mount: md })
    repl.feed("Path('/data/hello.txt').write_text('version2')", { mount: md })
    const result = repl.feed("Path('/data/hello.txt').read_text()", { mount: md })
    t.is(result, 'version2')
    // Original host file unchanged
    t.is(fs.readFileSync(path.join(dir, 'hello.txt'), 'utf-8'), 'hello world')
  } finally {
    cleanup()
  }
})

test('REPL overlay delete persists across feeds', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    repl.feed("Path('/data/hello.txt').unlink()", { mount: md })
    const result = repl.feed("Path('/data/hello.txt').exists()", { mount: md })
    t.is(result, false)
    // Host file still exists
    t.true(fs.existsSync(path.join(dir, 'hello.txt')))
  } finally {
    cleanup()
  }
})

test('REPL overlay mkdir and nested write persist', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    repl.feed("Path('/data/mydir').mkdir()", { mount: md })
    repl.feed("Path('/data/mydir/file.txt').write_text('nested')", { mount: md })
    const result = repl.feed("Path('/data/mydir/file.txt').read_text()", { mount: md })
    t.is(result, 'nested')
    t.false(fs.existsSync(path.join(dir, 'mydir')))
  } finally {
    cleanup()
  }
})

test('REPL overlay iterdir sees overlay files', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    repl.feed("Path('/data/extra.txt').write_text('extra')", { mount: md })
    const result = repl.feed("sorted([p.name for p in Path('/data').iterdir()])", { mount: md })
    t.deepEqual(result, ['data.bin', 'extra.txt', 'hello.txt', 'subdir'])
  } finally {
    cleanup()
  }
})

test('REPL overlay shared between REPL and Monty.run()', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'overlay' })
    // Write via REPL
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    repl.feed("Path('/data/shared.txt').write_text('from repl')", { mount: md })
    // Read via Monty.run()
    const result = new Monty("from pathlib import Path; Path('/data/shared.txt').read_text()").run({ mount: md })
    t.is(result, 'from repl')
  } finally {
    cleanup()
  }
})

test('REPL read-write mount writes to host', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-write' })
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    repl.feed("Path('/data/rw_file.txt').write_text('written')", { mount: md })
    const result = repl.feed("Path('/data/rw_file.txt').read_text()", { mount: md })
    t.is(result, 'written')
    // Host was actually modified
    t.is(fs.readFileSync(path.join(dir, 'rw_file.txt'), 'utf-8'), 'written')
  } finally {
    cleanup()
  }
})

test('REPL read-only mount blocks write', (t) => {
  const { dir, cleanup } = createTestDir()
  try {
    const md = new MountDirectory('/data', dir, { mode: 'read-only' })
    const repl = new MontyRepl()
    repl.feed('from pathlib import Path', { mount: md })
    const error = t.throws(() => repl.feed("Path('/data/nope.txt').write_text('x')", { mount: md }), {
      instanceOf: MontyRuntimeError,
    })
    t.true(error.message.includes('Read-only file system'))
  } finally {
    cleanup()
  }
})
