const unavailable = () => {
  throw new Error('node builtins are not available in browser tests')
}

export const Buffer = Uint8Array
export const spawnSync = unavailable
export const mkdtemp = unavailable
export const readFile = unavailable
export const rm = unavailable
export const writeFile = unavailable
export const mkdtempSync = unavailable
export const writeFileSync = unavailable
export const readFileSync = unavailable
export const rmSync = unavailable
export const existsSync = unavailable
export const mkdirSync = unavailable
export const tmpdir = unavailable
export const join = (...parts: string[]) => parts.join('/')
export const dirname = join
export const basename = (path: string) => path.split('/').at(-1) ?? ''
