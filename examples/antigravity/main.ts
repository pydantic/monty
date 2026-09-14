/// <reference types="vite/client" />
// The host half of the antigravity example.
//
// Monty's sandbox cannot touch the page, so the split is: the sandbox owns the
// physics and this file owns the SVG. Each animation frame feeds one
// expression into the live session and applies the offset it returns, which
// works because a session keeps its state between feeds — `flight` is created
// once, by the first feed, and survives every tick after it.

import { Monty, MontyError, MontyRuntimeError, MontySyntaxError, type MontySession } from '@pydantic/monty/wasm'

import sandboxSource from './antigravity.py?raw'

/** Points kept in the flight trail; older ones are dropped. */
const TRAIL_LENGTH = 400

/** Distance the figure must travel before the trail records another point. */
const TRAIL_SPACING = 0.5

/** The figure's feet in `antigravity.svg`, where the trail starts. */
const ORIGIN = { x: 60, y: 146 }

const codeInput = element<HTMLTextAreaElement>('code')
const launchButton = element<HTMLButtonElement>('launch')
const statusText = element('status')
const logPane = element('log')

/**
 * Bumped by every launch. The flight loop checks it each frame and stops when
 * it no longer matches, so a relaunch cannot leave two loops driving the SVG.
 */
let flightId = 0

const { char, trail } = await loadScene()
const trailPoints: { x: number; y: number }[] = []
codeInput.value = sandboxSource

const pool = await Monty.create({
  maxProcesses: 1,
  // Edited code is run as-is, so a runaway turn has to be preemptable: the
  // worker is terminated and replaced after this many seconds.
  requestTimeout: 10,
})
launchButton.disabled = false
launchButton.addEventListener('click', () => void launch())
statusText.textContent = 'ready'
await launch()

/** Runs the code in the editor from the start in a fresh session. */
async function launch(): Promise<void> {
  const id = ++flightId
  launchButton.disabled = true
  logPane.textContent = ''
  resetScene()

  // A fresh session per launch, so nothing from the previous flight is in
  // scope and the old worker's state goes away with it.
  const session = await pool.checkout({ limits: { maxDurationSecs: 5 } })
  try {
    // The seed is the one thing the flight cannot produce itself: the sandbox
    // has no clock, no entropy and no `random` module.
    const seed = Math.floor(Math.random() * 2 ** 31)
    log(`seed ${seed}`)
    await session.feedRun(codeInput.value, { inputs: { seed }, printCallback: onPrint })
    // The editor is usable again as soon as the code is in: clicking launch
    // mid-flight supersedes this flight rather than waiting for it.
    launchButton.disabled = false
    await fly(session, id)
  } catch (error) {
    report(error)
  } finally {
    await session.close()
    if (id === flightId) {
      launchButton.disabled = false
    }
  }
}

/** Ticks `flight.move()` once per frame until the flight is superseded. */
async function fly(session: MontySession, id: number): Promise<void> {
  let ticks = 0
  let feedTime = 0
  while (id === flightId) {
    const started = performance.now()
    const offset = await session.feedRun('flight.move()', { printCallback: onPrint })
    if (id !== flightId) {
      // Superseded while the feed was in flight; the new launch owns the SVG.
      break
    }
    feedTime += performance.now() - started
    ticks += 1

    const [x, y] = offsetFrom(offset)
    char.setAttribute('transform', `translate(${x} ${y})`)
    extendTrail(x, y)
    statusText.textContent = `${ticks} ticks, ${(feedTime / ticks).toFixed(2)} ms per feed`
    await nextFrame()
  }
}

/** Validates what the sandbox returned, which is whatever the editor said. */
function offsetFrom(value: unknown): [number, number] {
  const ok =
    Array.isArray(value) &&
    value.length === 2 &&
    value.every((part) => typeof part === 'number' && Number.isFinite(part))
  if (!ok) {
    throw new Error(`flight.move() must return two finite numbers, got ${JSON.stringify(value) ?? typeof value}`)
  }
  return value as [number, number]
}

/** Fetches the SVG and puts it on the page, as the PyScript example does. */
async function loadScene(): Promise<{ char: SVGGElement; trail: SVGPolylineElement }> {
  const response = await fetch(new URL('./antigravity.svg', import.meta.url))
  const parsed = new DOMParser().parseFromString(await response.text(), 'image/svg+xml')
  element('canvas').append(parsed.documentElement)
  return { char: element<SVGGElement>('char'), trail: element<SVGPolylineElement>('trail') }
}

function resetScene(): void {
  char.removeAttribute('transform')
  trailPoints.length = 0
  trail.setAttribute('points', '')
}

/**
 * Draws where the figure has been; the points are all sandbox output.
 *
 * Sampled by distance rather than by frame, so hovering in one place does not
 * spend the whole trail and a long climb still fits in it.
 */
function extendTrail(x: number, y: number): void {
  const point = { x: ORIGIN.x + x, y: ORIGIN.y + y }
  const last = trailPoints.at(-1)
  if (last !== undefined && Math.hypot(point.x - last.x, point.y - last.y) < TRAIL_SPACING) {
    return
  }
  trailPoints.push(point)
  if (trailPoints.length > TRAIL_LENGTH) {
    trailPoints.shift()
  }
  trail.setAttribute('points', trailPoints.map((p) => `${p.x.toFixed(2)},${p.y.toFixed(2)}`).join(' '))
}

/** Resolves on the next animation frame. */
function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()))
}

/** Routes sandbox `print()` output into the output pane. */
function onPrint(stream: 'stdout' | 'stderr', text: string): void {
  log(text.replace(/\n$/, ''), stream)
}

/** Ends the flight and shows why. */
function report(error: unknown): void {
  statusText.textContent = 'stopped'
  log(describe(error), 'stderr')
}

/** Renders an error for the output pane, with a traceback where there is one. */
function describe(error: unknown): string {
  if (error instanceof MontyRuntimeError || error instanceof MontySyntaxError) {
    return error.display('traceback')
  }
  if (error instanceof MontyError) {
    return error.display('type-msg')
  }
  return String(error)
}

function log(text: string, stream: 'stdout' | 'stderr' = 'stdout'): void {
  const line = document.createElement('div')
  line.className = stream
  line.textContent = text
  logPane.append(line)
  logPane.scrollTop = logPane.scrollHeight
}

/** Looks up an element the markup is expected to carry. */
function element<T extends Element = HTMLElement>(id: string): T {
  const found = document.getElementById(id)
  if (found === null) {
    throw new Error(`missing #${id} in index.html`)
  }
  return found as unknown as T
}
