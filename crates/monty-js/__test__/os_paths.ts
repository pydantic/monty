// Filesystem path checks shared by the native and wasm specs: relative results,
// NUL rejection and no-handler error spelling. Only types come from the package
// so `vitest.wasm.config.ts`, which runs without the napi addon, can load this
// module; each spec supplies its own backend's `MontyFileHandle`.

import type { FeedOptions, MontyFileHandle } from '@pydantic/monty'
import { t } from './assertions.js'

/** Runs one snippet in a fresh session and returns its result. */
export type Run = (code: string, options?: FeedOptions) => Promise<unknown>

/** Checks relative Python results while callbacks continue to receive absolute paths. */
export async function checkRelativePathResults(run: Run, FileHandle: typeof MontyFileHandle): Promise<void> {
  const calls: unknown[] = []
  const result = await run(
    `from pathlib import Path
([str(p) for p in Path('.').iterdir()],
 [str(p) for p in Path('sub/..').iterdir()],
 open('./file.txt').name,
 Path('./file.txt').open().name,
 str(open(b'./file.txt').name))`,
    {
      cwd: '/data',
      os: (name, args) => {
        calls.push([name, args])
        if (name === 'Path.iterdir') return ['/data/file.txt']
        if (name === 'open') return new FileHandle(args[0] as string, 'r')
        throw new Error(`unexpected OS call: ${name}`)
      },
    },
  )
  t.deepEqual(result, [['file.txt'], ['sub/../file.txt'], './file.txt', 'file.txt', "b'./file.txt'"])
  t.deepEqual(calls, [
    ['Path.iterdir', ['/data']],
    ['Path.iterdir', ['/data']],
    ['open', ['/data/file.txt', 'r']],
    ['open', ['/data/file.txt', 'r']],
    ['open', ['/data/file.txt', 'r']],
  ])
}

/** Checks path rejection and error spelling through native and WASM OS callbacks. */
export async function checkOsPathValidation(run: Run): Promise<void> {
  const calls: unknown[] = []
  const result = await run(
    `import os
from pathlib import Path
errors = []
for operation in [
    lambda: open(path),
    lambda: Path(path).read_text(),
    lambda: os.chdir(path),
    lambda: os.rename(path, 'dst'),
    lambda: os.rename('src', path),
]:
    try:
        operation()
    except ValueError as e:
        errors.append(str(e))
p = Path(path)
(errors, p.exists(), p.is_file(), p.is_dir(), p.is_symlink(), os.getcwd())`,
    {
      cwd: '/data',
      inputs: { path: 'bad\0/../x' },
      os: (...args) => {
        calls.push(args)
        return true
      },
    },
  )
  t.deepEqual(result, [
    [
      'embedded null byte',
      'embedded null byte',
      'chdir: embedded null character in path',
      'rename: embedded null character in src',
      'rename: embedded null character in dst',
    ],
    false,
    false,
    false,
    false,
    '/data',
  ])
  t.deepEqual(calls, [])
  for (const [code, expected] of [
    ['import os\nos.listdir()', "PermissionError: Permission denied: '/'"],
    ["open('./x')", "PermissionError: Permission denied: '/x'"],
    ["open('')", "PermissionError: Permission denied: ''"],
    ["open('bad\\0/../x')", 'ValueError: embedded null byte'],
  ] as const) {
    const error = await t.throwsAsync(() => run(code, { cwd: '/' }))
    t.is(error.message, expected)
  }
}
