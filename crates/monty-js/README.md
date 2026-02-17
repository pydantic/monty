# @pydantic/monty

JavaScript/TypeScript bindings for the Monty sandboxed Python interpreter.

## Installation

```bash
npm install @pydantic/monty
```

## Basic Usage

```ts
import { Monty } from '@pydantic/monty'

// Create interpreter and run code
const m = new Monty('1 + 2')
const result = m.run() // returns 3
```

## Input Variables

```ts
const m = new Monty('x + y', { inputs: ['x', 'y'] })
const result = m.run({ inputs: { x: 10, y: 20 } }) // returns 30
```

## External Functions

For synchronous external functions, pass them directly to `run()`:

```ts
const m = new Monty('add(2, 3)', { externalFunctions: ['add'] })

const result = m.run({
  externalFunctions: {
    add: (a: number, b: number) => a + b,
  },
}) // returns 5
```

For async external functions, use `runMontyAsync()`:

```ts
import { Monty, runMontyAsync } from '@pydantic/monty'

const m = new Monty('fetch_data(url)', {
  inputs: ['url'],
  externalFunctions: ['fetch_data'],
})

const result = await runMontyAsync(m, {
  inputs: { url: 'https://example.com' },
  externalFunctions: {
    fetch_data: async (url: string) => {
      const response = await fetch(url)
      return response.text()
    },
  },
})
```

## Iterative Execution

For fine-grained control over external function calls, use `start()` and `resume()`:

```ts
const m = new Monty('a() + b()', { externalFunctions: ['a', 'b'] })

let progress = m.start()
while (progress instanceof MontySnapshot) {
  console.log(`Calling: ${progress.functionName}`)
  console.log(`Args: ${progress.args}`)
  // Provide the return value and resume
  progress = progress.resume({ returnValue: 10 })
}
// progress is now MontyComplete
console.log(progress.output) // 20
```

## Error Handling

```ts
import { Monty, MontySyntaxError, MontyRuntimeError, MontyTypingError } from '@pydantic/monty'

try {
  const m = new Monty('1 / 0')
  m.run()
} catch (error) {
  if (error instanceof MontySyntaxError) {
    console.log('Syntax error:', error.message)
  } else if (error instanceof MontyRuntimeError) {
    console.log('Runtime error:', error.message)
    console.log('Traceback:', error.traceback())
  } else if (error instanceof MontyTypingError) {
    console.log('Type error:', error.displayDiagnostics())
  }
}
```

## Type Checking

```ts
const m = new Monty('"hello" + 1')
try {
  m.typeCheck()
} catch (error) {
  if (error instanceof MontyTypingError) {
    console.log(error.displayDiagnostics('concise'))
  }
}

// Or enable during construction
const m2 = new Monty('1 + 1', { typeCheck: true })
```

## Resource Limits

```ts
const m = new Monty('1 + 1')
const result = m.run({
  limits: {
    maxAllocations: 10000,
    maxDurationSecs: 5,
    maxMemory: 1024 * 1024, // 1MB
    maxRecursionDepth: 100,
  },
})
```

## Serialization

```ts
// Save parsed code to avoid re-parsing
const m = new Monty('complex_code()')
const data = m.dump()

// Later, restore without re-parsing
const m2 = Monty.load(data)
const result = m2.run()

// Snapshots can also be serialized
const snapshot = m.start()
if (snapshot instanceof MontySnapshot) {
  const snapshotData = snapshot.dump()
  // Later, restore and resume
  const restored = MontySnapshot.load(snapshotData)
  const result = restored.resume({ returnValue: 42 })
}
```

## API Reference

### `Monty` Class

- `constructor(code: string, options?: MontyOptions)` - Parse Python code
- `run(options?: RunOptions)` - Execute and return the result
- `start(options?: StartOptions)` - Start iterative execution
- `typeCheck(prefixCode?: string)` - Perform static type checking
- `dump()` - Serialize to binary format
- `Monty.load(data)` - Deserialize from binary format
- `scriptName` - The script name (default: `'main.py'`)
- `inputs` - Declared input variable names
- `externalFunctions` - Declared external function names

### `MontyOptions`

- `scriptName?: string` - Name used in tracebacks (default: `'main.py'`)
- `inputs?: string[]` - Input variable names
- `externalFunctions?: string[]` - External function names
- `typeCheck?: boolean` - Enable type checking on construction
- `typeCheckPrefixCode?: string` - Code to prepend for type checking

### `RunOptions`

- `inputs?: object` - Input variable values
- `limits?: ResourceLimits` - Resource limits
- `externalFunctions?: object` - External function callbacks

### `ResourceLimits`

- `maxAllocations?: number` - Maximum heap allocations
- `maxDurationSecs?: number` - Maximum execution time in seconds
- `maxMemory?: number` - Maximum heap memory in bytes
- `gcInterval?: number` - Run GC every N allocations
- `maxRecursionDepth?: number` - Maximum call stack depth (default: 1000)

### `MontySnapshot` Class

Returned by `start()` when execution pauses at an external function call.

- `scriptName` - The script being executed
- `functionName` - The external function being called
- `args` - Positional arguments
- `kwargs` - Keyword arguments
- `resume(options: ResumeOptions)` - Resume with return value or exception
- `dump()` / `MontySnapshot.load(data)` - Serialization

### `MontyComplete` Class

Returned by `start()` or `resume()` when execution completes.

- `output` - The final result value

### Error Classes

- `MontyError` - Base class for all Monty errors
- `MontySyntaxError` - Syntax/parsing errors
- `MontyRuntimeError` - Runtime exceptions (with `traceback()`)
- `MontyTypingError` - Type checking errors (with `displayDiagnostics()`)
