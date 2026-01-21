# @pydantic/monty

JavaScript bindings for the Monty sandboxed Python interpreter.

## Installation

```bash
npm install @pydantic/monty
```

## Usage (CommonJS)

```js
const monty = require('@pydantic/monty')

const { output, result } = monty.run('print("hello")\n1 + 2')
console.log(output) // "hello\n"
console.log(result) // debug representation of the final value
```

## Usage (ESM / TypeScript)

```ts
import monty from '@pydantic/monty'

const res = monty.run('print("hi")\n3 * 7')
console.log(res.output)
console.log(res.result)
```

## API

- `run(code: string): { output: string, result: string }` — execute Python code
  in a sandboxed Monty VM. `output` contains captured `print()` output; `result`
  is the debug (`{:?}`) representation of the last expression's value.
