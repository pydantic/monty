# Install for JavaScript

```bash
npm install @pydantic/monty
```

Under Node the package is a native (napi) binding over the same Rust worker pool the Python package uses.
The binding and the `monty` worker binary ship as platform-specific packages selected through `optionalDependencies`, so
a plain `npm install` gets you everything.

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout()
console.log(await session.feedRun('1 + 2')) // 3
```

Continue with the [JavaScript quickstart](../quickstart/javascript.md).

## Browsers and WebAssembly

For browsers, or anywhere subprocesses are impossible, the same package exposes an in-process WebAssembly build under
the `@pydantic/monty/wasm` subpath.
A bundler resolving the `browser` condition on the main entry point gets that build automatically.
See the [JavaScript quickstart](../quickstart/javascript.md#browsers-and-webassembly) for what differs there.
