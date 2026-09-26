/// <reference types="vite/client" />
// xkcd 353 in the browser: the PyScript antigravity program, running in a Monty
// sandbox. `random` and `time.sleep` are the sandbox's own; the DOM is not, so
// the three names the program takes from PyScript — `pydom`, `DOMParser` and
// `open_url` — are provided by this file instead. Everything the program does
// to the page goes through them, and they allow only what the flight needs.

import { ClassInstance, Monty } from '@pydantic/monty/wasm'

import antigravityPy from './antigravity.py?raw'

const canvas = document.getElementById('canvas')!
const status = document.getElementById('status')!
let started = ''

// ---- What the sandbox gets in place of its imports ----

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
    tick()
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

/** The program sets the figure's transform once per tick, so counting them gives the frame rate. */
let ticks = 0
let flightStartedAt = 0

function tick(): void {
  ticks += 1
  if (ticks === 1) {
    flightStartedAt = performance.now()
    return
  }
  const fps = ((ticks - 1) / ((performance.now() - flightStartedAt) / 1000)).toFixed(0)
  status.textContent = `${started} · ${fps} fps`
}

// ---- Running the program ----

try {
  const svgText = await (await fetch(new URL('./antigravity.svg', import.meta.url))).text()

  const startedAt = performance.now()
  await using pool = await Monty.create()
  // Every call from the sandbox to a host object, and every `time.sleep`, is a
  // "suspension"; the default budget of 1000 would end the flight after a few
  // hundred ticks.
  await using session = await pool.checkout({ limits: { maxSuspensions: 100_000 } })
  started = `started in ${(performance.now() - startedAt).toFixed(0)} ms`
  status.textContent = started

  const hostFunctions = {
    // `from pyodide.http import open_url`: the one file this page will serve.
    open_url: (url: string): ClassInstance => {
      if (url !== './antigravity.svg') throw new Error(`open_url will not fetch ${url}`)
      return expose(new Response(svgText))
    },
  }

  // Running the file builds `_auto`, which fetches, parses and appends the SVG.
  await session.feedRun(antigravityPy, {
    inputs: {
      DOMParser: expose(new DomParser()),
      // `from pyweb import pydom`: `pydom["body"][0]` is where the SVG is appended.
      pydom: { body: [node(canvas)] },
    },
    externalLookup: hostFunctions,
  })
  // What PyScript's main.py calls. The whole flight is this one feed: `fly`
  // moves the figure and sleeps 10 ms, over and over, until the suspension
  // budget runs out. Each sleep suspends the sandbox and the worker waits it
  // out in the page, so the tab stays responsive.
  await session.feedRun('fly()', { externalLookup: hostFunctions })
} catch (error) {
  status.textContent = `stopped: ${error}`
  console.error(error)
}
