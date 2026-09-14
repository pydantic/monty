# antigravity in the browser

[xkcd 353](https://xkcd.com/353/) as a Monty example, running the Python from the
[PyScript version](https://github.com/pyscript/examples/tree/main/antigravity) unchanged.
It runs in a Monty sandbox — WebAssembly, in a Web Worker, in the page — and drives the comic from in there.
The code is in an editable panel, so a visitor can rewrite the physics and launch again.

## Running it

The wasm component is not checked in, so build it first from the repository root:

```bash
make build-wasm
cd examples/antigravity && npm install && npm run dev
```

Vite serves the page and emits the component's wasm assets and the worker entry as their own chunks.
`npm run build` produces a static bundle of the same thing, and `npm run typecheck` checks the host half.

## What the port changes

[`antigravity.py`](antigravity.py) is the PyScript file with its five imports replaced by a comment, and nothing
else changed. Those names come from the host instead, as [host objects](../../docs/host-objects.md) and
[host functions](../../docs/host-functions.md) that [`main.ts`](main.ts) passes in:

| Import                                          | What the host passes                                                         |
| ----------------------------------------------- | ---------------------------------------------------------------------------- |
| `from pyweb import pydom`                       | a dict with one entry, `body`, holding the element the sandbox may append to |
| `from js import DOMParser`                      | an object whose `new()` returns a parser that accepts `image/svg+xml` only   |
| `from pyodide.http import open_url`             | a function serving files the host loaded up front, by the name asked for     |
| `from pyodide.ffi.wrappers import set_interval` | a function that records the interval; the host ticks `_auto.move()` itself   |
| `import random`                                 | an object with `normalvariate`, over the host's `Math.random`                |

So the flight runs as it does under PyScript — `open_url` fetches the SVG, `DOMParser` parses it, `pydom` appends it,
and `move()` sets a `transform` on the figure once per tick — but every one of those calls suspends the sandbox and is
answered by [`main.ts`](main.ts), which decides what each of them is allowed to do.
The dashed trail is the host's own: it draws a point wherever the `transform` puts the figure.

Two places where Monty differs:

- A sandbox callable cannot cross to the host, so `set_interval(self.move, 10)` hands the host a marker rather than
    something it can call. The host honours the interval it was given by feeding `_auto.move()` instead.
- `open_url` is not awaited by the program, so the host cannot answer it with a promise. The files it will serve are
    fetched before the first feed, which is also the whole of its access policy.

Feeding one call per tick works because a session keeps its state between feeds: `_auto` is built by the first feed and
every later feed sees the same object, along with the host objects it is holding.
The page reports the average time a feed takes.

## The sandbox boundary

Everything in the editor is untrusted code, and the sandbox reaches the page only through what this file hands it:

- `setAttribute` accepts `transform` and nothing else, so no attribute that runs script can be set.
- `open_url` serves the files the host loaded, so the sandbox cannot make the page fetch anything else.
- `parseFromString` parses SVG, and `append` takes nodes the host itself produced.
- Each launch takes a fresh session, so a previous flight leaves nothing behind.
- `maxDurationSecs` stops a runaway loop inside the sandbox, and `requestTimeout` terminates the whole Web Worker if
    the turn never comes back at all.
- Every DOM call is a suspension charged against `maxSuspensions`, which is what bounds the length of a flight.

The line art in [`antigravity.svg`](antigravity.svg) is the trace of the comic from the PyScript example, copied
verbatim; the dashed flight trail is this example's. xkcd is CC BY-NC 2.5, Randall Munroe.
