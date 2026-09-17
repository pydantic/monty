// Runs every ```ts fence in docs/ and README.md as its own module, the counterpart of the
// pytest-examples runner on the Python side: a snippet must type-check and run, its printed
// output is not compared. `test="skip"` on the fence skips running but not type-checking.
import { spawnSync } from 'node:child_process'
import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { expect, test } from 'vitest'

const PACKAGE = fileURLToPath(new URL('..', import.meta.url))
const REPO = join(PACKAGE, '..', '..')
const OUT = join(PACKAGE, '__test__', 'docs') // gitignored

// the opening indent is required on the closing fence and stripped from the body, so a
// fence inside a `=== "TypeScript"` tab is found and dedented, as pytest-examples does
const FENCE = /^( *)```(?:ts|typescript)\b([^\n]*)\n([\s\S]*?)^\1```/gm
const SKIP = /test=(['"])(.+?)\1/

interface Snippet {
  name: string
  file: string
  skip: string | null
}

function* markdownFiles(): Generator<string> {
  yield join(REPO, 'README.md')
  const docs = join(REPO, 'docs')
  for (const entry of readdirSync(docs, { recursive: true }) as string[]) {
    // docs/api/rust is generated, and the API pages hold no examples
    if (entry.endsWith('.md') && !entry.startsWith('api')) yield join(docs, entry)
  }
}

/** Writes each fence to `OUT` as a standalone `.ts` module and describes it. */
function extract(): Snippet[] {
  const snippets: Snippet[] = []
  for (const path of markdownFiles()) {
    const page = relative(REPO, path).split(sep).join('/')
    const source = readFileSync(path, 'utf8')
    for (const match of source.matchAll(FENCE)) {
      const [, indent, info, body] = match
      const line = source.slice(0, match.index).split('\n').length
      const code = body
        .split('\n')
        .map((l) => (l.startsWith(indent) ? l.slice(indent.length) : l))
        .join('\n')
      // mirrors the page's directory so two pages can never map to one file
      const file = join(OUT, `${page.replace(/\.md$/, '')}__${line}.ts`)
      mkdirSync(dirname(file), { recursive: true })
      writeFileSync(file, code)
      snippets.push({ name: `${page}:${line}`, file, skip: SKIP.exec(info)?.[2] ?? null })
    }
  }
  return snippets
}

// generated at module load: vitest collects tests synchronously
rmSync(OUT, { recursive: true, force: true })
mkdirSync(OUT, { recursive: true })
writeFileSync(
  join(OUT, 'tsconfig.json'),
  JSON.stringify({
    // the specs' config resolves `@pydantic/monty` to the sources but excludes this
    // directory; a docs example may leave a parameter or an import unused
    extends: '../tsconfig.json',
    compilerOptions: { noUnusedLocals: false, noUnusedParameters: false },
    include: ['.'],
    exclude: [],
  }),
)
const snippets = extract()

test('snippet files are distinct', () => {
  expect(new Set(snippets.map((s) => s.file)).size).toBe(snippets.length)
})

test('docs snippets type-check', () => {
  // the js entry via process.execPath: `.bin/tsc` is a .cmd shim on Windows
  const tsc = join(PACKAGE, 'node_modules', 'typescript', 'bin', 'tsc')
  const result = spawnSync(process.execPath, [tsc, '-p', OUT], { encoding: 'utf8' })
  if (result.status !== 0) throw new Error(result.stdout + result.stderr)
})

for (const { name, file, skip } of snippets) {
  if (skip === null) {
    test(name, async () => {
      await import(/* @vite-ignore */ pathToFileURL(file).href)
    })
  } else {
    test.skip(`${name} (${skip})`, () => {})
  }
}
