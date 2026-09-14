# antigravity in the browser

[xkcd 353](https://xkcd.com/353/) as a Monty example, ported from the
[PyScript version](https://github.com/pyscript/examples/tree/main/antigravity).
Python computes the flight path in a Monty sandbox — WebAssembly, in a Web Worker, in the page — and the page draws it.
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

The PyScript version runs Python with the DOM in reach: it fetches the SVG with `pyodide.http.open_url`, appends the
parsed node through `pydom`, and calls `pyodide.ffi.wrappers.set_interval` to drive itself.
None of that is available to sandboxed code in Monty, which is the point of the sandbox — it can compute, and the host
decides what any of it means.
So the example splits in two:

| PyScript                                           | Monty                                                                           |
| -------------------------------------------------- | ------------------------------------------------------------------------------- |
| `move()` sets `transform` on an SVG node           | `move()` returns `(x, y)` and [`main.ts`](main.ts) sets the attribute           |
| `set_interval(self.move, 10)` from inside Python   | the host drives `requestAnimationFrame`, feeding `flight.move()` once per frame |
| `open_url("./antigravity.svg")` from inside Python | the host fetches the SVG                                                        |
| `random.normalvariate(0, 1)`                       | a generator written in Python, seeded by the host                               |

Feeding one expression per frame works because a session keeps its state between feeds: `flight` is built by the first
feed and every later feed sees the same object.
The page reports the average time a feed takes.

`random` is not one of Monty's [modules](../../docs/limitations/modules.md), and a sandbox has no clock and no other source
of entropy, so [`antigravity.py`](antigravity.py) carries its own MINSTD generator and Box-Muller transform and the
host passes in a seed as an input.
The same flight comes back for the same seed.

## The sandbox boundary

Everything in the editor is untrusted code:

- The sandbox cannot reach the page, the network or the filesystem, so nothing in the editor can, whatever it says.
- Each launch takes a fresh session, so a previous flight leaves nothing behind.
- `maxDurationSecs` stops a runaway loop inside the sandbox, and `requestTimeout` terminates the whole Web Worker if
    the turn never comes back at all.
- `flight.move()` returns whatever the edited code decides to return, so the host checks the shape of it before
    touching the SVG.

The stick figure in [`antigravity.svg`](antigravity.svg) is drawn for this example, in the spirit of the comic rather
than traced from it.
