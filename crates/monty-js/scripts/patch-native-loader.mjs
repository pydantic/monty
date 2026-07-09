import { appendFileSync, readFileSync, writeFileSync } from 'node:fs'

const jsPath = 'index.js'
const dtsPath = 'index.d.ts'

const nativeExports = ['MAX_VALUE_DEPTH', 'NativeMount', 'NativePool', 'NativeSession']

let js = readFileSync(jsPath, 'utf8')
js = js.replace(/const \{ ([^}]+) \} = nativeBinding/, (match, exports) => {
  const names = exports.split(',').map((name) => name.trim())
  for (const name of nativeExports) {
    if (!names.includes(name)) {
      names.push(name)
    }
  }
  return `const { ${names.join(', ')} } = nativeBinding`
})

for (const name of nativeExports) {
  if (!js.includes(`export { ${name} }`)) {
    js += `\nexport { ${name} }\n`
  }
}
writeFileSync(jsPath, js)

const dts = readFileSync(dtsPath, 'utf8')
if (!dts.includes('export declare class NativePool')) {
  appendFileSync(
    dtsPath,
    `\nexport declare const MAX_VALUE_DEPTH: number\nexport declare class NativeMount {}\nexport declare class NativePool {\n  constructor(options?: unknown)\n  start(): Promise<void>\n  checkout(options?: unknown): NativeSession\n  close(): Promise<void>\n}\nexport declare class NativeSession {\n  readonly workerPid?: number\n  [key: string]: any\n}\n`,
  )
}
