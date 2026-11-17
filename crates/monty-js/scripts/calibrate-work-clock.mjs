// Calibrates the work clock against the wasm build under Node, where clocks work.
//
// Runs each `monty-bench` calibration script in the built component twice: once on the work
// clock for the time it reports, once on the wall clock for the time it took. Their ratio times
// the current `NANOS_PER_UNIT` is the measured rate; the median over the scripts is what the
// constant should be. `--check` fails when a script is more than MAX_DRIFT from the constant.
//
// Needs `npm run build:wasm && npm run build:ts` first; `make calibrate-work-clock` does both.

import { readdirSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { instantiateWorker } from '../dist/worker/host.js'
import { loadModule } from '../dist/worker/loadModule.node.js'

const pkg = dirname(dirname(fileURLToPath(import.meta.url)))
const workspace = dirname(dirname(pkg))
const scriptsDir = join(workspace, 'crates', 'monty-bench', 'benches', 'python', 'work_clock')
const meterSource = join(workspace, 'crates', 'monty-types', 'src', 'resource', 'work_meter.rs')

/** Timed runs per script; the fastest is the least disturbed. */
const RUNS = 7
/** How far a script's rate may sit from `NANOS_PER_UNIT` before `--check` fails. */
const MAX_DRIFT = 3

const nanosPerUnit = Number(/pub const NANOS_PER_UNIT: u64 = (\d+);/.exec(readFileSync(meterSource, 'utf8'))[1])
const modules = await loadModule()

/** Runs `code` in a fresh component on `executionClock` and returns its execution microseconds. */
async function executionMicros(name, code, executionClock) {
  const dispatch = await instantiateWorker(modules)
  const configured = dispatch({
    tag: 'configure',
    val: { scriptName: name, executionClock, typeCheck: false, typeCheckFormat: 'full', typeCheckColor: false },
  })
  if (configured.events.at(-1).tag !== 'ok') throw new Error(`${name}: configure failed`)
  const result = dispatch({
    tag: 'feed',
    val: { code, inputs: [], values: { nodes: [] }, skipTypeCheck: true, cwd: '/' },
  })
  const event = result.events.at(-1)
  if (event.tag !== 'complete') throw new Error(`${name}: ${JSON.stringify(event.val)}`)
  return Number(result.feedExecutionMicros)
}

const rows = []
for (const file of readdirSync(scriptsDir).filter((file) => file.endsWith('.py')).sort()) {
  const code = readFileSync(join(scriptsDir, file), 'utf8')
  const name = file.slice(0, -3)
  const workMicros = await executionMicros(name, code, 'work')
  let wallMicros = Infinity
  for (let run = 0; run < RUNS; run++) wallMicros = Math.min(wallMicros, await executionMicros(name, code, 'wall'))
  const units = (workMicros * 1000) / nanosPerUnit
  rows.push({ name, units, wallMicros, rate: (wallMicros * 1000) / units })
}
rows.sort((a, b) => a.rate - b.rate)
const median = rows[Math.floor(rows.length / 2)].rate

console.log(`${'script'.padEnd(18)} ${'units'.padStart(12)} ${'time'.padStart(10)} ${'ns/unit'.padStart(9)} ${'/const'.padStart(8)}`)
let drifted = false
for (const row of rows) {
  const ratio = row.rate / nanosPerUnit
  const ok = ratio >= 1 / MAX_DRIFT && ratio <= MAX_DRIFT
  drifted ||= !ok
  console.log(
    `${row.name.padEnd(18)} ${row.units.toFixed(0).padStart(12)} ${(row.wallMicros / 1000).toFixed(2).padStart(8)}ms ` +
      `${row.rate.toFixed(2).padStart(9)} ${ratio.toFixed(2).padStart(8)}${ok ? '' : '  <-- drift'}`,
  )
}
console.log(`wasm median: ${median.toFixed(2)} ns/unit (NANOS_PER_UNIT is ${nanosPerUnit})`)
if (process.argv.includes('--check') && drifted) {
  console.error(`a script is more than ${MAX_DRIFT}x from NANOS_PER_UNIT`)
  process.exit(1)
}
