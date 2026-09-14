/// <reference types="vite/client" />
// The host half of the antigravity example.
//
// The sandbox runs the PyScript program with its imports taken away, so every
// name it used to import is something this file hands it: two host functions
// and three host objects, each a policy saying what the sandbox may call. The
// program still thinks it is driving the page — but `setAttribute` lands here
// first, and what it is allowed to do is decided on this side of the boundary.

import {
  ClassInstance,
  Monty,
  MontyError,
  MontyRuntimeError,
  MontySyntaxError,
  type MontySession,
} from '@pydantic/monty/wasm'

import sandboxSource from './antigravity.py?raw'

/** Points kept in the flight trail; older ones are dropped. */
const TRAIL_LENGTH = 400

/** Distance the figure must travel before the trail records another point. */
const TRAIL_SPACING = 0.4

/** The figure's start, in the layer coordinates `#trail` is drawn in. */
const ORIGIN = { x: 167.2, y: 131 }

/** The only attribute the sandbox may set, on the only nodes it is given. */
const WRITABLE_ATTRIBUTE = 'transform'

const codeInput = element<HTMLTextAreaElement>('code')
const launchButton = element<HTMLButtonElement>('launch')
const statusText = element('status')
const logPane = element('log')
const canvas = element('canvas')

/**
 * Bumped by every launch. The flight loop checks it each frame and stops when
 * it no longer matches, so a relaunch cannot leave two loops driving the SVG.
 */
let flightId = 0

/** The trail polyline of the SVG the sandbox appended, once it has appended one. */
let trail: SVGPolylineElement | null = null
const trailPoints: { x: number; y: number }[] = []

/** Set when the sandbox asks to be ticked, with the interval it asked for. */
let tick: { intervalMs: number } | null = null

/** The pool of workers, created by `main()` at the foot of this file. */
let pool: Awaited<ReturnType<typeof Monty.create>>

/** What `open_url` will serve, by the name the sandbox asks for. */
const assets = new Map<string, string>()

/** Boots the sandbox, fills the editor and flies. */
async function main(): Promise<void> {
  codeInput.value = sandboxSource
  const svg = await fetch(new URL('./antigravity.svg', import.meta.url))
  assets.set('./antigravity.svg', await svg.text())
  pool = await Monty.create({
    maxProcesses: 1,
    // Edited code is run as-is, so a runaway turn has to be preemptible: the
    // worker is terminated and replaced after this many seconds.
    requestTimeout: 10,
  })
  launchButton.disabled = false
  launchButton.addEventListener('click', () => void launch())
  statusText.textContent = 'ready'
  await launch()
}

/** Runs the code in the editor from the start in a fresh session. */
async function launch(): Promise<void> {
  const id = ++flightId
  launchButton.disabled = true
  logPane.textContent = ''
  resetScene()

  // A fresh session per launch, so nothing from the previous flight is in
  // scope and the old worker's state goes away with it.
  const session = await pool.checkout({
    limits: {
      maxDurationSecs: 5,
      // Every DOM call the flight makes is a round trip to this file, and each
      // one is charged here: a tick costs four, so this is the flight's length.
      maxSuspensions: 200_000,
    },
  })
  try {
    // Importing the module builds `_auto`, which fetches the SVG through
    // `open_url` and appends it to the page — all of it from inside the sandbox.
    await session.feedRun(codeInput.value, { inputs: hostObjects(), externalLookup: hostFunctions(), ...printing })
    // `fly()` is what PyScript's main.py calls; here it reaches `set_interval`.
    await session.feedRun('fly()', { externalLookup: hostFunctions(), ...printing })
    // The editor is usable again as soon as the flight is airborne: clicking
    // launch mid-flight supersedes this flight rather than waiting for it.
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

/**
 * Ticks the flight until it is superseded.
 *
 * A sandbox callable cannot cross to the host — `self.move` reaches
 * `set_interval` as a marker, not as something callable — so the interval the
 * sandbox asked for is honoured here, against the object it left behind.
 */
async function fly(session: MontySession, id: number): Promise<void> {
  let ticks = 0
  let feedTime = 0
  while (id === flightId && tick !== null) {
    const started = performance.now()
    await session.feedRun('_auto.move()', { externalLookup: hostFunctions(), ...printing })
    if (id !== flightId) {
      // Superseded while the feed was in flight; the new launch owns the page.
      break
    }
    feedTime += performance.now() - started
    ticks += 1
    statusText.textContent = `${ticks} ticks, ${(feedTime / ticks).toFixed(2)} ms per feed`
    await delay(tick.intervalMs)
  }
}

/** The objects standing in for the module's imports, bound by the first feed. */
function hostObjects(): Record<string, unknown> {
  return {
    // `pydom[...]` is a subscript, and a host object is not subscriptable, so
    // this is a dict — with the one selector the program actually looks up.
    pydom: { body: [toSandbox(sceneNode(canvas))] },
    DOMParser: toSandbox(new DomParsers()),
    random: toSandbox(new Random()),
  }
}

/** The functions standing in for the module's imports, resolved by name. */
function hostFunctions(): Record<string, unknown> {
  return { open_url: openUrl, set_interval: setIntervalRequest }
}

/**
 * Serves a file the sandbox asks for by name, out of what the host loaded up front.
 *
 * The program calls `open_url` without awaiting it, so the answer has to be
 * ready: an async host function would hand the sandbox a coroutine. Loading the
 * files first is also the whole access policy — the sandbox names a file, and
 * gets it only if the host had already decided to serve it.
 */
function openUrl(url: unknown): unknown {
  const text = typeof url === 'string' ? assets.get(url) : undefined
  if (text === undefined) {
    throw new Error(`open_url has nothing for ${String(url)}; this page serves ${[...assets.keys()].join(', ')}`)
  }
  return toSandbox(new UrlResponse(text))
}

/** Records the sandbox's request to be ticked; `fly()` above services it. */
function setIntervalRequest(_callback: unknown, intervalMs: unknown): void {
  if (typeof intervalMs !== 'number' || !Number.isFinite(intervalMs) || intervalMs < 0) {
    throw new TypeError(`set_interval expected a positive number of milliseconds, got ${String(intervalMs)}`)
  }
  tick = { intervalMs }
}

/** What `open_url` returns: the fetched text, and nothing else. */
class UrlResponse {
  constructor(private readonly text: string) {}

  read(): string {
    return this.text
  }
}

/** The `js.DOMParser` stand-in, whose `new()` the program calls. */
class DomParsers {
  new(): Parser {
    return new Parser()
  }
}

/** Parses what the sandbox fetched, as SVG and nothing else. */
class Parser {
  parseFromString(text: unknown, mimeType: unknown): ParsedDocument {
    if (typeof text !== 'string') {
      throw new TypeError(`parseFromString expected a string, got ${typeof text}`)
    }
    if (mimeType !== 'image/svg+xml') {
      throw new Error(`parseFromString refused ${String(mimeType)}: this page parses SVG only`)
    }
    const parsed = new DOMParser().parseFromString(text, 'image/svg+xml')
    const root = parsed.documentElement
    if (root.tagName !== 'svg') {
      throw new Error(`parseFromString got ${root.tagName}, not an SVG document`)
    }
    return new ParsedDocument(sceneNode(root))
  }
}

/** A parsed document, exposing the one attribute the program reads. */
class ParsedDocument {
  constructor(readonly documentElement: SceneNode) {}
}

/**
 * The whole DOM surface the sandbox gets: three methods on nodes the host
 * chose to hand out.
 *
 * A node reaches sandbox code only as a wrapper around one of these, so the
 * program can walk from the SVG root to the figure and move it, and cannot
 * reach the rest of the page, the attributes that run script, or the document.
 */
class SceneNode {
  constructor(readonly node: Element) {}

  /** Returns a list: a host object cannot be subscripted, so `[1]` needs one. */
  getElementsByTagName(tag: unknown): SceneNode[] {
    if (typeof tag !== 'string') {
      throw new TypeError(`getElementsByTagName expected a string, got ${typeof tag}`)
    }
    return [...this.node.getElementsByTagName(tag)].map(sceneNode)
  }

  /** Moves the figure, and draws where it has been. */
  setAttribute(name: unknown, value: unknown): void {
    if (name !== WRITABLE_ATTRIBUTE) {
      throw new Error(`setAttribute refused ${String(name)}: only ${WRITABLE_ATTRIBUTE} is writable`)
    }
    if (typeof value !== 'string') {
      throw new TypeError(`setAttribute expected a string, got ${typeof value}`)
    }
    this.node.setAttribute(WRITABLE_ATTRIBUTE, value)
    extendTrail(value)
  }

  /** Puts the parsed SVG on the page, which is where the trail comes from. */
  append(child: unknown): void {
    if (!(child instanceof SceneNode)) {
      throw new TypeError('append expected a node this page handed out')
    }
    this.node.append(child.node)
    trail = child.node.querySelector('#trail')
  }
}

/** One `SceneNode` per element, so a tick reuses the wrapper it made last time. */
const nodes = new WeakMap<Element, SceneNode>()

/** Returns the node for `target`, making it on first use. */
function sceneNode(target: Element): SceneNode {
  const existing = nodes.get(target)
  if (existing !== undefined) {
    return existing
  }
  const node = new SceneNode(target)
  nodes.set(target, node)
  return node
}

/** The `random` stand-in: the host has entropy, a sandbox has none. */
class Random {
  /** `random.normalvariate`, by the Box-Muller transform. */
  normalvariate(mu: unknown, sigma: unknown): number {
    if (typeof mu !== 'number' || typeof sigma !== 'number') {
      throw new TypeError('normalvariate expected two numbers')
    }
    const radius = Math.sqrt(-2 * Math.log(1 - Math.random()))
    return mu + sigma * radius * Math.cos(2 * Math.PI * Math.random())
  }
}

/** The policy each host class is exposed under: these methods, nothing else. */
const POLICIES = new Map<unknown, { methods: string[]; attrs?: string[] }>([
  [UrlResponse, { methods: ['read'] }],
  [DomParsers, { methods: ['new'] }],
  [Parser, { methods: ['parseFromString'] }],
  [ParsedDocument, { methods: [], attrs: ['documentElement'] }],
  [SceneNode, { methods: ['append', 'getElementsByTagName', 'setAttribute'] }],
  [Random, { methods: ['normalvariate'] }],
])

/** One wrapper per object, so repeated ticks do not fill the instance store. */
const wrappers = new WeakMap<object, ClassInstance>()

/**
 * Wraps a host object for the sandbox under its class's policy.
 *
 * Nothing crosses the boundary unwrapped: this is also the `convertValue` hook,
 * so a method's return value is wrapped the same way, and a value belonging to
 * no policy stays a plain value or fails conversion in the sandbox.
 */
function toSandbox(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(toSandbox)
  }
  if (typeof value !== 'object' || value === null) {
    return value
  }
  const policy = POLICIES.get(value.constructor)
  if (policy === undefined) {
    return value
  }
  const existing = wrappers.get(value)
  if (existing !== undefined) {
    return existing
  }
  const wrapper = new ClassInstance(value, {
    allowedMethods: policy.methods,
    lazyAttrs: policy.attrs ?? [],
    convertValue: (_name, inner) => toSandbox(inner),
  })
  wrappers.set(value, wrapper)
  return wrapper
}

/** Clears the page of the last flight; the next one appends its own SVG. */
function resetScene(): void {
  canvas.replaceChildren()
  trail = null
  trailPoints.length = 0
  tick = null
}

/**
 * Draws where the figure has been, from the transform the sandbox set.
 *
 * Sampled by distance rather than by tick, so hovering in one place does not
 * spend the whole trail and a long climb still fits in it. A transform the
 * host cannot read is applied but not drawn — editing `move()` is allowed to
 * produce something this does not understand.
 */
function extendTrail(transform: string): void {
  const match = /^translate\(\s*(-?[\d.e+-]+)\s*,\s*(-?[\d.e+-]+)\s*\)$/.exec(transform)
  if (trail === null || match === null) {
    return
  }
  const x = ORIGIN.x + Number(match[1])
  const y = ORIGIN.y + Number(match[2])
  if (!Number.isFinite(x) || !Number.isFinite(y)) {
    return
  }
  const last = trailPoints.at(-1)
  if (last !== undefined && Math.hypot(x - last.x, y - last.y) < TRAIL_SPACING) {
    return
  }
  trailPoints.push({ x, y })
  if (trailPoints.length > TRAIL_LENGTH) {
    trailPoints.shift()
  }
  trail.setAttribute('points', trailPoints.map((p) => `${p.x.toFixed(2)},${p.y.toFixed(2)}`).join(' '))
}

/** Resolves after `ms`, on an animation frame so a hidden tab stops flying. */
function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(() => requestAnimationFrame(() => resolve()), ms))
}

/** Routes sandbox `print()` output into the output pane. */
const printing = {
  printCallback: (stream: 'stdout' | 'stderr', text: string): void => log(text.replace(/\n$/, ''), stream),
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

// Last, because the classes above are not hoisted and `main()` reaches them.
await main()
