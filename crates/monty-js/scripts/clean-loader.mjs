import { rmSync } from 'node:fs'

for (const file of ['index.js', 'index.d.ts']) {
  rmSync(file, { force: true })
}
