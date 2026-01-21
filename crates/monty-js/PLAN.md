# monty-js Implementation Plan

This document outlines the plan to bring `monty-js` to feature parity with `monty-python`.

## Current State

The `monty-js` package currently has a minimal API:

```typescript
export function run(code: string): RunResult

export interface RunResult {
  output: string // captured print output
  result: string // debug repr of result
}
```

## Target API

We need to match the `monty-python` API, adapted for TypeScript/JavaScript conventions.

---

## Phase 1: Core Classes and Basic Execution

### 1.1 `Monty` Class

The main interpreter class that parses code once and can be run multiple times.

```typescript
export class Monty {
  constructor(code: string, options?: MontyOptions)

  run(options?: RunOptions): unknown
  typeCheck(prefixCode?: string): void
  dump(): Uint8Array
  static load(data: Uint8Array, options?: LoadOptions): Monty
}

export interface MontyOptions {
  scriptName?: string // default: 'main.py'
  inputs?: string[] // input variable names
  externalFunctions?: string[] // external function names
  typeCheck?: boolean // run type checking on init
  typeCheckPrefixCode?: string // code to prepend for type checking
}

export interface RunOptions {
  inputs?: Record<string, unknown>
  limits?: ResourceLimits
  externalFunctions?: Record<string, (...args: unknown[]) => unknown>
  printCallback?: (stream: 'stdout', text: string) => void
}

export interface LoadOptions {
  // For future dataclass-like support if needed
}
```

### 1.2 `ResourceLimits` Interface

```typescript
export interface ResourceLimits {
  maxAllocations?: number // max heap allocations
  maxDurationSecs?: number // max execution time in seconds
  maxMemory?: number // max heap memory in bytes
  gcInterval?: number // run GC every N allocations
  maxRecursionDepth?: number // max call stack depth (default: 1000)
}
```

### 1.3 Rust Implementation Tasks

- [ ] Create `PyMonty` equivalent struct with napi bindings
- [ ] Implement constructor that parses code and stores `MontyRun`
- [ ] Implement `run()` method with options handling
- [ ] Implement `typeCheck()` method
- [ ] Implement `dump()`/`load()` serialization

---

## Phase 2: Exception Hierarchy

### 2.1 Exception Classes

```typescript
export class MontyError extends Error {
  /** Returns the inner exception type name and message */
  get exception(): { type: string; message: string }
}

export class MontySyntaxError extends MontyError {
  display(format?: 'type-msg' | 'msg'): string
}

export class MontyRuntimeError extends MontyError {
  traceback(): Frame[]
  display(format?: 'traceback' | 'type-msg' | 'msg'): string
}

export class MontyTypingError extends MontyError {
  display(
    format?: 'full' | 'concise' | 'azure' | 'json' | 'jsonlines' | 'rdjson' | 'pylint' | 'gitlab' | 'github',
    color?: boolean,
  ): string
}
```

### 2.2 `Frame` Class

```typescript
export class Frame {
  readonly filename: string
  readonly line: number
  readonly column: number
  readonly endLine: number
  readonly endColumn: number
  readonly functionName: string | null
  readonly sourceLine: string | null

  toObject(): FrameObject
}

export interface FrameObject {
  filename: string
  line: number
  column: number
  endLine: number
  endColumn: number
  functionName: string | null
  sourceLine: string | null
}
```

### 2.3 Rust Implementation Tasks

- [ ] Create `MontyError` base class with napi
- [ ] Create `MontySyntaxError` with `display()` method
- [ ] Create `MontyRuntimeError` with `traceback()` and `display()` methods
- [ ] Create `MontyTypingError` with `display()` method
- [ ] Create `Frame` class with all properties

---

## Phase 3: Iterative Execution (External Functions)

### 3.1 `MontySnapshot` Class

Represents paused execution at an external function call.

```typescript
export class MontySnapshot {
  readonly scriptName: string
  readonly functionName: string
  readonly args: unknown[]
  readonly kwargs: Record<string, unknown>

  resume(options: { returnValue: unknown }): MontySnapshot | MontyComplete
  resume(options: { exception: Error }): MontySnapshot | MontyComplete

  dump(): Uint8Array
  static load(data: Uint8Array, options?: SnapshotLoadOptions): MontySnapshot
}

export interface SnapshotLoadOptions {
  printCallback?: (stream: 'stdout', text: string) => void
}
```

### 3.2 `MontyComplete` Class

Represents completed execution.

```typescript
export class MontyComplete {
  readonly output: unknown
}
```

### 3.3 Add `start()` to `Monty`

```typescript
export class Monty {
  // ... existing methods ...

  start(options?: StartOptions): MontySnapshot | MontyComplete
}

export interface StartOptions {
  inputs?: Record<string, unknown>
  limits?: ResourceLimits
  printCallback?: (stream: 'stdout', text: string) => void
}
```

### 3.4 Rust Implementation Tasks

- [ ] Create `MontySnapshot` struct with napi bindings
- [ ] Implement `resume()` with return value or exception
- [ ] Implement `dump()`/`load()` for snapshot serialization
- [ ] Create `MontyComplete` struct
- [ ] Implement `start()` method on `Monty`

---

## Phase 4: Type Conversion

### 4.1 JavaScript to Monty Value Conversion

Support converting these JS types to Monty values:

| JavaScript Type    | Monty Type     |
| ------------------ | -------------- |
| `null`             | `None`         |
| `boolean`          | `bool`         |
| `number` (integer) | `int`          |
| `number` (float)   | `float`        |
| `bigint`           | `int` (BigInt) |
| `string`           | `str`          |
| `Uint8Array`       | `bytes`        |
| `Array`            | `list`         |
| `Object` (plain)   | `dict`         |
| `Set`              | `set`          |
| `Map`              | `dict`         |

### 4.2 Monty Value to JavaScript Conversion

| Monty Type    | JavaScript Type                   |
| ------------- | --------------------------------- |
| `None`        | `null`                            |
| `bool`        | `boolean`                         |
| `int` (small) | `number`                          |
| `int` (big)   | `bigint`                          |
| `float`       | `number`                          |
| `str`         | `string`                          |
| `bytes`       | `Uint8Array`                      |
| `list`        | `Array`                           |
| `tuple`       | `Array` (with `__tuple__: true`?) |
| `dict`        | `Object` or `Map`                 |
| `set`         | `Set`                             |
| `frozenset`   | `Set` (with `__frozen__: true`?)  |

### 4.3 Rust Implementation Tasks

- [ ] Create `convert.rs` module for JS <-> Monty value conversion
- [ ] Handle nested structures recursively
- [ ] Handle edge cases (circular references, special values)
- [ ] Consider tuple/frozenset representation options

---

## Phase 5: Testing

### 5.1 Test Categories

Mirror the Python test structure:

- [ ] Basic execution tests
- [ ] Input variable tests
- [ ] External function tests
- [ ] Resource limit tests
- [ ] Print callback tests
- [ ] Exception handling tests
- [ ] Serialization tests
- [ ] Type conversion tests
- [ ] Iterative execution tests

### 5.2 Test File Structure

```
__test__/
  basic.spec.ts        # Basic run() tests
  monty-class.spec.ts  # Monty class tests
  exceptions.spec.ts   # Exception hierarchy tests
  external.spec.ts     # External function tests
  limits.spec.ts       # Resource limits tests
  conversion.spec.ts   # Type conversion tests
  serialization.spec.ts # dump/load tests
```

---

## Phase 6: Documentation and Polish

### 6.1 Documentation Tasks

- [ ] Update `index.d.ts` with comprehensive JSDoc comments
- [ ] Update `README.md` with usage examples
- [ ] Add inline code examples

### 6.2 API Refinements

- [ ] Consider async variants for long-running code
- [ ] Consider streaming print output via callbacks
- [ ] Ensure error messages are helpful

---

## Implementation Order

Recommended order of implementation:

1. **Phase 1.1-1.2**: `Monty` class with basic `run()` (no external functions)
2. **Phase 2**: Exception hierarchy (needed for proper error handling)
3. **Phase 4**: Type conversion (needed for inputs/outputs)
4. **Phase 1.3**: Complete `Monty` class (`typeCheck`, `dump`/`load`)
5. **Phase 3**: Iterative execution (`start`/`resume`, `MontySnapshot`, `MontyComplete`)
6. **Phase 5**: Comprehensive testing
7. **Phase 6**: Documentation and polish

---

## Open Questions

1. **Tuple representation**: Should tuples be represented as arrays with a marker property, or as a custom class?

2. **Async API**: Should we provide Promise-based variants of `run()` and `start()` for better Node.js integration?

3. **Error inheritance**: In JS, custom error classes are tricky. Should we use class inheritance or composition for the error hierarchy?

4. **BigInt handling**: Should large integers always become `bigint`, or only when they exceed `Number.MAX_SAFE_INTEGER`?

5. **Dataclass support**: The Python API has dataclass registry support. Is there an equivalent need for TypeScript (e.g., class instances)?

---

## File Structure

```
crates/monty-js/
  src/
    lib.rs           # Module root and napi exports
    monty.rs         # Monty class implementation
    snapshot.rs      # MontySnapshot and MontyComplete
    exceptions.rs    # Exception hierarchy
    frame.rs         # Frame class
    convert.rs       # Type conversion utilities
    limits.rs        # ResourceLimits handling
  index.d.ts         # TypeScript type definitions
  index.js           # Generated JS bindings
  __test__/
    *.spec.ts        # Test files
```

---

## References

- `crates/monty-python/monty.pyi` - Python type stubs (source of truth for API)
- `crates/monty-python/src/` - Python binding implementation
- `crates/monty/src/lib.rs` - Core Monty API
