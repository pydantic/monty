// Calibrates the work clock against the wasm build under Node, where clocks work.
//
// Every run is made twice in the built component: once on the work clock for the time it reports,
// once on the wall clock for the time it took. Their ratio times the current `NANOS_PER_UNIT` is
// the measured rate. Three reports, as in `crates/monty-bench/benches/work_clock.rs`:
//
// - categories (always): the median over the `monty-bench` calibration scripts is what the
//   constant should be; `--check` fails when a script is more than MAX_DRIFT from the constant.
// - `--constructs`: units charged per construct against the units its time is worth.
// - `--corpus`: the spread over the interpreter's self-contained test cases.
//
// `--fit-data <dir>` instead times every script `monty-bench` wrote under `<dir>/scripts` and writes
// `<dir>/wasm.json`, the times `make fit-work-clock` fits the weights to.
//
// Needs `npm run build:wasm && npm run build:ts` first; `make calibrate-work-clock` does both.

import { readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { instantiateWorker } from '../dist/worker/host.js'
import { loadModule } from '../dist/worker/loadModule.node.js'

const pkg = dirname(dirname(fileURLToPath(import.meta.url)))
const workspace = dirname(dirname(pkg))
const scriptsDir = join(workspace, 'crates', 'monty-bench', 'benches', 'python', 'work_clock')
const constructsFile = join(workspace, 'crates', 'monty-bench', 'benches', 'python', 'work_clock_constructs.py')
const corpusDir = join(workspace, 'crates', 'monty', 'test_cases')
const meterSource = join(workspace, 'crates', 'monty-types', 'src', 'resource', 'work_meter.rs')

/** Timed runs per script; the fastest is the least disturbed. */
const RUNS = 7
/** How far a script's rate may sit from `NANOS_PER_UNIT` before `--check` fails. */
const MAX_DRIFT = 3

/** Units below which a corpus case is too short for a microsecond clock. */
const CORPUS_MIN_UNITS = 20_000
/** Work-clock budget for a corpus case, so one that never ends is skipped, not waited for. */
const CORPUS_BUDGET_SECONDS = 2
/** Corpus cases listed at each end of the rate distribution. */
const CORPUS_OUTLIERS = 15

// The wasm build's constant: the one under `cfg(target_family = "wasm")`.
const nanosPerUnit = Number(
  /#\[cfg\(target_family = "wasm"\)\]\npub const NANOS_PER_UNIT: u64 = (\d+);/.exec(
    readFileSync(meterSource, 'utf8'),
  )[1],
)
const modules = await loadModule()

/** Runs `code` in a fresh component on `executionClock` and returns its execution microseconds, raised or not. */
async function executionMicros(name, code, executionClock, limits) {
  const dispatch = await instantiateWorker(modules)
  const configured = dispatch({
    tag: 'configure',
    val: { scriptName: name, executionClock, limits, typeCheck: false, typeCheckFormat: 'full', typeCheckColor: false },
  })
  if (configured.events.at(-1).tag !== 'ok') throw new Error(`${name}: configure failed`)
  const result = dispatch({
    tag: 'feed',
    val: { code, inputs: [], values: { nodes: [] }, skipTypeCheck: true, cwd: '/' },
  })
  return { micros: Number(result.feedExecutionMicros), event: result.events.at(-1) }
}

/** Measures `code`: units charged and its fastest wall time. `budgetSeconds` bounds the work-clock run only. */
async function measure(name, code, budgetSeconds) {
  const limits = budgetSeconds === undefined ? undefined : { maxFeedDurationMicros: BigInt(budgetSeconds * 1e6) }
  const work = await executionMicros(name, code, 'work', limits)
  let wallMicros = Infinity
  if (work.micros > 0 && !(budgetSeconds && work.micros >= budgetSeconds * 1e6)) {
    for (let run = 0; run < RUNS; run++) {
      wallMicros = Math.min(wallMicros, (await executionMicros(name, code, 'wall')).micros)
    }
  }
  const units = (work.micros * 1000) / nanosPerUnit
  return { name, units, wallMicros, rate: (wallMicros * 1000) / units, event: work.event }
}

/** Every `*.py` file in `dir` as `[name, source]`, in name order. */
function pythonFiles(dir) {
  return readdirSync(dir)
    .filter((file) => file.endsWith('.py'))
    .sort()
    .map((file) => [file.slice(0, -3), readFileSync(join(dir, file), 'utf8')])
}

const fitData = process.argv.indexOf('--fit-data')
if (fitData !== -1) {
  const dir = process.argv[fitData + 1]
  const times = {}
  for (const [name, code] of pythonFiles(join(dir, 'scripts'))) {
    try {
      const row = await measure(name, code)
      times[name] = { wallMicros: row.wallMicros, units: row.units }
    } catch (error) {
      // A case that exhausts the wasm stack traps the component; the fit leaves it out.
      console.log(`skipping ${name}: ${error.message}`)
    }
  }
  writeFileSync(join(dir, 'wasm.json'), JSON.stringify({ nanosPerUnit, times }))
  console.log(`wrote ${Object.keys(times).length} times to ${join(dir, 'wasm.json')}`)
  process.exit(0)
}

const rows = []
for (const [name, code] of pythonFiles(scriptsDir)) {
  const row = await measure(name, code)
  if (row.event.tag !== 'complete') throw new Error(`${name}: ${JSON.stringify(row.event.val)}`)
  rows.push(row)
}
rows.sort((a, b) => a.rate - b.rate)
const median = rows[Math.floor(rows.length / 2)].rate

console.log(
  `${'script'.padEnd(18)} ${'units'.padStart(12)} ${'time'.padStart(10)} ${'ns/unit'.padStart(9)} ${'/const'.padStart(8)}`,
)
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

if (process.argv.includes('--constructs')) {
  // Sections start at `# === name ===`; the first is the prelude, the second the baseline.
  const [, prelude, ...sections] = readFileSync(constructsFile, 'utf8').split(/^# === (?=.* ===$)/m)
  const preludeCode = prelude.slice(prelude.indexOf('\n'))
  const iterations = Number(/^N = ([\d_]+)$/m.exec(preludeCode)[1].replaceAll('_', ''))
  let baseline
  console.log(
    `\n${'construct'.padEnd(22)} ${'charged'.padStart(9)} ${'measured'.padStart(9)} ${'meas/chg'.padStart(9)}`,
  )
  for (const section of sections) {
    const name = section.slice(0, section.indexOf(' ==='))
    const row = await measure(name, preludeCode + section.slice(section.indexOf('\n')))
    if (row.event.tag !== 'complete') throw new Error(`${name}: ${JSON.stringify(row.event.val)}`)
    if (baseline === undefined) {
      baseline = row
      continue
    }
    const charged = (row.units - baseline.units) / iterations
    const measured = ((row.wallMicros - baseline.wallMicros) * 1000) / iterations / nanosPerUnit
    // A construct charged what the baseline is has no ratio to report.
    const ratio = charged >= 0.5 ? (measured / charged).toFixed(2) : '-'
    console.log(
      `${name.padEnd(22)} ${charged.toFixed(1).padStart(9)} ${measured.toFixed(1).padStart(9)} ${ratio.padStart(9)}`,
    )
  }
  console.log(`units per iteration beyond \`baseline\`; measured is the extra time at ${nanosPerUnit} ns/unit`)
}

if (process.argv.includes('--corpus')) {
  const all = []
  for (const [name, code] of pythonFiles(corpusDir)) {
    // Cases that need a host (external calls, an event loop, a mount) cannot run here.
    if (/^# (call-external|run-async|mount-fs) = true$/m.test(code)) continue
    try {
      const row = await measure(name, code, CORPUS_BUDGET_SECONDS)
      if (Number.isFinite(row.wallMicros)) all.push(row)
    } catch (error) {
      // A case that exhausts the wasm stack traps the component; each run has its own.
      console.log(`skipping ${name}: ${error.message}`)
    }
  }
  const units = all.reduce((sum, row) => sum + row.units, 0)
  const micros = all.reduce((sum, row) => sum + row.wallMicros, 0)
  const timed = all.filter((row) => row.units >= CORPUS_MIN_UNITS).sort((a, b) => a.rate - b.rate)
  const quantile = (q) => (timed[Math.floor((timed.length - 1) * q)].rate / nanosPerUnit).toFixed(2)
  console.log(
    `\ncorpus: ${all.length} cases, ${units.toFixed(0)} units in ${(micros / 1000).toFixed(1)}ms, ` +
      `${((micros * 1000) / units / nanosPerUnit).toFixed(2)} of NANOS_PER_UNIT overall`,
  )
  console.log(
    `${timed.length} cases of ${CORPUS_MIN_UNITS}+ units, rate / NANOS_PER_UNIT: p5 ${quantile(0.05)}  ` +
      `p25 ${quantile(0.25)}  p50 ${quantile(0.5)}  p75 ${quantile(0.75)}  p95 ${quantile(0.95)}`,
  )
  const outliers = Math.min(CORPUS_OUTLIERS, Math.floor(timed.length / 2))
  for (const [title, list] of [
    ['charged too much', timed.slice(0, outliers)],
    ['charged too little', timed.slice(timed.length - outliers)],
  ]) {
    console.log(`${title}:`)
    for (const row of list) {
      console.log(
        `  ${row.name.padEnd(44)} ${row.units.toFixed(0).padStart(10)} ${(row.rate / nanosPerUnit).toFixed(2).padStart(8)}`,
      )
    }
  }
}

if (process.argv.includes('--check') && drifted) {
  console.error(`a script is more than ${MAX_DRIFT}x from NANOS_PER_UNIT`)
  process.exit(1)
}
