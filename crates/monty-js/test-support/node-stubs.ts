export class FunctionSnapshot {}
export class FutureSnapshot {}
export class NameLookupSnapshot {}

export class MountDir {
  constructor() {
    throw new Error('@pydantic/monty/node is not available in browser tests')
  }
}

export const findMontyBinary = () => {
  throw new Error('@pydantic/monty/node is not available in browser tests')
}

export const flushTelemetry = () => {
  throw new Error('Node telemetry is not available in browser tests')
}

export const instrumentTelemetry = () => {
  throw new Error('Node telemetry is not available in browser tests')
}

export class MontyInstrumentation {
  constructor() {
    throw new Error('Node telemetry is not available in browser tests')
  }
}

export * from '@pydantic/monty'
