import { afterAll, beforeAll, expect, test as vitestTest } from 'vitest'
import type { TestOptions } from 'vitest'

export type ErrorConstructor<T extends Error = Error> = new (...args: never[]) => T
export type ExecutionContext = Assertions
export type TestFn = AvaCompatTest

interface ThrowsOptions {
  instanceOf?: ErrorConstructor
  message?: string | RegExp
}

interface Assertions {
  is: (actual: unknown, expected: unknown, message?: string) => void
  not: (actual: unknown, expected: unknown, message?: string) => void
  deepEqual: (actual: unknown, expected: unknown, message?: string) => void
  true: (actual: unknown, message?: string) => void
  false: (actual: unknown, message?: string) => void
  truthy: (actual: unknown, message?: string) => void
  regex: (actual: string, regex: RegExp, message?: string) => void
  throws: <T extends Error = Error>(fn: () => unknown, options?: ThrowsOptions) => T
  throwsAsync: <T extends Error = Error>(
    fn: (() => unknown | Promise<unknown>) | Promise<unknown>,
    options?: ThrowsOptions,
  ) => Promise<T>
  notThrows: (fn: () => unknown, message?: string) => void
  fail: (message?: string) => never
  pass: () => void
}

type TestImplementation = (
  title: string,
  fn: (t: Assertions) => unknown | Promise<unknown>,
  options?: TestOptions,
) => void

interface AvaCompatTest extends TestImplementation {
  skip: TestImplementation
  before: (fn: () => unknown | Promise<unknown>) => void
  after: ((fn: () => unknown | Promise<unknown>) => void) & { always: (fn: () => unknown | Promise<unknown>) => void }
}

const assertions: Assertions = {
  is: (actual, expected, message) => expect(actual, message).toBe(expected),
  not: (actual, expected, message) => expect(actual, message).not.toBe(expected),
  deepEqual: (actual, expected, message) => expect(actual, message).toEqual(expected),
  true: (actual, message) => expect(actual, message).toBe(true),
  false: (actual, message) => expect(actual, message).toBe(false),
  truthy: (actual, message) => expect(actual, message).toBeTruthy(),
  regex: (actual, regex, message) => expect(actual, message).toMatch(regex),
  throws: (fn, options) => {
    try {
      fn()
    } catch (error) {
      checkError(error, options)
      return error as Error
    }
    throw new Error('Function did not throw')
  },
  throwsAsync: async (fn, options) => {
    try {
      await (typeof fn === 'function' ? fn() : fn)
    } catch (error) {
      checkError(error, options)
      return error as Error
    }
    throw new Error('Function did not throw')
  },
  notThrows: (fn, message) => expect(fn, message).not.toThrow(),
  fail: (message) => {
    throw new Error(message ?? 'Test failed')
  },
  pass: () => {},
}

const test = ((title, fn, options) => vitestTest(title, options ?? {}, () => fn(assertions))) as AvaCompatTest

test.skip = ((title, fn, options) => vitestTest.skip(title, options ?? {}, () => fn(assertions))) as TestImplementation
test.before = (fn) => beforeAll(fn)
test.after = Object.assign((fn: () => unknown | Promise<unknown>) => afterAll(fn), {
  always: (fn: () => unknown | Promise<unknown>) => afterAll(fn),
})

export default test

function checkError(error: unknown, options: ThrowsOptions | undefined): void {
  if (options?.instanceOf !== undefined) {
    expect(error).toBeInstanceOf(options.instanceOf)
  }
  if (options?.message !== undefined) {
    const message = error instanceof Error ? error.message : String(error)
    if (typeof options.message === 'string') {
      expect(message).toBe(options.message)
    } else {
      expect(message).toMatch(options.message)
    }
  }
}
