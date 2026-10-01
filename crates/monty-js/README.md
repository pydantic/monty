# @pydantic/monty

Run Python in Monty's sandbox from JavaScript or TypeScript.
Node uses subprocess workers; `@pydantic/monty/wasm` uses Node worker threads or browser Web Workers.

```sh
npm install @pydantic/monty
```

```ts
import { Monty } from '@pydantic/monty'

await using pool = await Monty.create()
await using session = await pool.checkout({ limits: { maxMemory: 10_000_000, maxFeedDurationSecs: 1 } })
console.log(await session.feedRun('x * 2', { inputs: { x: 21 } })) // 42
```

- [JavaScript API and examples](https://pydantic.dev/docs/monty/quickstart/javascript/)
- [Security and isolation guarantees](https://pydantic.dev/docs/monty/concepts/security/)
- [Supported Python subset](https://pydantic.dev/docs/monty/limitations/)
- [Source and contributing](https://github.com/pydantic/monty)
