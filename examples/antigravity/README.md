# antigravity in the browser

[xkcd 353](https://xkcd.com/353/), running the Python from the
[PyScript example](https://github.com/pyscript/examples/tree/main/antigravity) in a Monty sandbox in the browser.
Monty is compiled to WebAssembly and runs in a Web Worker; opening the page starts the flight.

![The comic, with the figure part way up its flight and the status line under it](antigravity-screenshot.png)

## Running it

The wasm component is not checked in, so build it first from the repository root:

```bash
make build-wasm
cd examples/antigravity && npm install && npm run dev
```

`npm run build` writes a static bundle to `dist/`.

## How it works

[`antigravity.py`](antigravity.py) is the PyScript file with two changes: of its five imports only `random` and `time`
remain, which Monty provides, and `fly()` loops over `self.move()` and `time.sleep()` instead of handing `self.move` to
`set_interval`, since a sandbox function cannot be passed to the host.

The sandbox has no DOM, so [`main.ts`](main.ts) provides the other three names: `DOMParser` and `pydom` as
[host objects](../../docs/host-objects.md), `open_url` as a [host function](../../docs/host-functions.md).
`Node` wraps a DOM element and allows `getElementsByTagName`, `append` and `setAttribute` of `transform` only.
`open_url` returns an SVG fetched before the sandbox starts, because the program does not `await` it.

`main.ts` feeds the file once, which builds `_auto` and appends the SVG, then feeds `fly()` once: that feed is the whole
flight, and it ends when `maxSuspensions` runs out.
The line under the comic shows the time to a checked-out session and the frame rate, each frame being one tick and
its 10 ms sleep.

The line art in [`antigravity.svg`](antigravity.svg) is copied from the PyScript example.
xkcd is CC BY-NC 2.5, Randall Munroe.
