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

export * from '@pydantic/monty'
