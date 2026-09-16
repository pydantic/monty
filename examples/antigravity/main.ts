/// <reference types="vite/client" />
// xkcd 353 in the browser: the PyScript antigravity program, running in a Monty
// sandbox. The sandbox has no DOM, so the five things the program imports —
// `random`, `pydom`, `DOMParser`, `open_url` and `set_interval` — are provided
// by this file instead. Everything the program does to the page goes through
// them, and they allow only what the flight needs.

import { ClassInstance, Monty } from '@pydantic/monty/wasm'

import antigravityPy from './antigravity.py?raw'

const canvas = document.getElementById('canvas')!
const status = document.getElementById('status')!

// ---- What the sandbox gets in place of its imports ----

/** `import random`: a sandbox has no randomness of its own, so the host provides it. */
class Random {
  normalvariate(mu: number, sigma: number): number {
    const radius = Math.sqrt(-2 * Math.log(1 - Math.random()))
    return mu + sigma * radius * Math.cos(2 * Math.PI * Math.random())
  }
}

/** A DOM element the sandbox may use. These three methods are all it gets. */
class Node {
  #element: Element

  constructor(element: Element) {
    this.#element = element
  }

  getElementsByTagName(tag: string): ClassInstance[] {
    return [...this.#element.getElementsByTagName(tag)].map(node)
  }

  setAttribute(name: string, value: string): void {
    if (name !== 'transform') throw new Error(`the sandbox may only set transform, not ${name}`)
    this.#element.setAttribute(name, value)
    drawTrail(value)
  }

  append(child: Node): void {
    this.#element.append(child.#element)
  }
}

/** `from js import DOMParser`: the program calls `DOMParser.new().parseFromString(text, mime)`. */
class DomParser {
  new(): ClassInstance {
    return expose(new DomParser())
  }

  parseFromString(text: string, mimeType: string): ClassInstance {
    const doc = new DOMParser().parseFromString(text, mimeType as DOMParserSupportedType)
    return expose(new SvgDocument(node(doc.documentElement)))
  }
}

/** What `parseFromString` returns: the program reads `.documentElement` from it. */
class SvgDocument {
  constructor(readonly documentElement: ClassInstance) {}
}

/** What `open_url` returns: the program calls `.read()` on it. */
class Response {
  #text: string

  constructor(text: string) {
    this.#text = text
  }

  read(): string {
    return this.#text
  }
}

/**
 * Puts a host object in front of the sandbox. The sandbox can call the methods
 * the class defines and read its public fields; private `#fields` stay here.
 */
function expose(object: object): ClassInstance {
  return new ClassInstance(object, { allowedMethods: 'all', eagerAttrs: 'all' })
}

/** One wrapper per element, because the program looks the figure up on every tick. */
const nodes = new WeakMap<Element, ClassInstance>()

function node(element: Element): ClassInstance {
  let wrapped = nodes.get(element)
  if (wrapped === undefined) nodes.set(element, (wrapped = expose(new Node(element))))
  return wrapped
}

/** Draws the flight path from the transforms the program sets on the figure. */
const trail: string[] = []

function drawTrail(transform: string): void {
  const match = /translate\((.+), (.+)\)/.exec(transform)
  if (match === null) return
  // (167.2, 131) is where the figure starts, in the SVG's own coordinates
  trail.push(`${167.2 + Number(match[1])},${131 + Number(match[2])}`)
  if (trail.length > 2000) trail.shift()
  canvas.querySelector('#trail')?.setAttribute('points', trail.join(' '))
}

// ---- Running the program ----

try {
  const svgText = await (await fetch(new URL('./antigravity.svg', import.meta.url))).text()

  const startedAt = performance.now()
  await using pool = await Monty.create()
  // Every call from the sandbox to a host object is a "suspension", and the
  // default budget of 1000 would end the flight after a few hundred ticks.
  await using session = await pool.checkout({ limits: { maxSuspensions: 100_000 } })
  const startMs = (performance.now() - startedAt).toFixed(0)
  status.textContent = `started in ${startMs} ms`

  // The rate `set_interval` asks for. The sandbox can't hand its `self.move`
  // callback to the host, so the host runs the loop itself instead.
  let intervalMs = 10

  const hostFunctions = {
    // `from pyodide.http import open_url`: the one file this page will serve.
    open_url: (url: string): ClassInstance => {
      if (url !== './antigravity.svg') throw new Error(`open_url will not fetch ${url}`)
      return expose(new Response(svgText))
    },
    // `from pyodide.ffi.wrappers import set_interval`
    set_interval: (_callback: unknown, ms: number): void => {
      intervalMs = ms
    },
  }

  // Running the file builds `_auto`, which fetches, parses and appends the SVG.
  await session.feedRun(antigravityPy, {
    inputs: {
      random: expose(new Random()),
      DOMParser: expose(new DomParser()),
      // `from pyweb import pydom`: `pydom["body"][0]` is where the SVG is appended.
      pydom: { body: [node(canvas)] },
    },
    externalLookup: hostFunctions,
  })
  // What PyScript's main.py calls; it reaches `set_interval` above.
  await session.feedRun('fly()', { externalLookup: hostFunctions })

  // The session keeps `_auto` alive between feeds, so each tick is one call.
  let feedTime = 0
  for (let ticks = 1; ; ticks++) {
    const before = performance.now()
    await session.feedRun('_auto.move()', { externalLookup: hostFunctions })
    feedTime += performance.now() - before
    status.textContent = `started in ${startMs} ms · ${ticks} ticks, ${(feedTime / ticks).toFixed(2)} ms per feed`
    await new Promise((resolve) => setTimeout(resolve, intervalMs))
  }
} catch (error) {
  status.textContent = `stopped: ${error}`
  console.error(error)
}
