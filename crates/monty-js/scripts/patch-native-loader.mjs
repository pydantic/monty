import { appendFileSync, readFileSync, writeFileSync } from 'node:fs'

const jsPath = 'index.js'
const dtsPath = 'index.d.ts'

let js = readFileSync(jsPath, 'utf8')
js = js.replace(
  'const { Monty, MontyComplete, MontyException, JsMontyException, MontyNameLookup, MontyRepl, MontySnapshot, MontyTypingError, MountDir } = nativeBinding',
  'const { MAX_VALUE_DEPTH, Monty, MontyComplete, MontyException, JsMontyException, MontyNameLookup, MontyRepl, MontySnapshot, MontyTypingError, MountDir, NativeMount, NativePool, NativeSession } = nativeBinding',
)
js = js.replace('export { Monty }', 'export { MAX_VALUE_DEPTH }\nexport { Monty }')
js = js.replace(
  'export { MountDir }',
  'export { MountDir }\nexport { NativeMount }\nexport { NativePool }\nexport { NativeSession }',
)
writeFileSync(jsPath, js)

appendFileSync(
  dtsPath,
  `\nexport declare const MAX_VALUE_DEPTH: number\nexport declare class NativeMount {}\nexport declare class NativePool {\n  constructor(options?: unknown)\n  start(): Promise<void>\n  checkout(options?: unknown): NativeSession\n  close(): Promise<void>\n}\nexport declare class NativeSession {\n  readonly workerPid?: number\n  [key: string]: any\n}\n`,
)
