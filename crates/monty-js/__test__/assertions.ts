import { expect } from 'vitest'

interface ThrowsOptions {
  instanceOf?: new (...args: never[]) => Error
  message?: string | RegExp
}

export const t = {
  is: (actual: unknown, expected: unknown) => expect(actual).toBe(expected),
  not: (actual: unknown, expected: unknown) => expect(actual).not.toBe(expected),
  deepEqual: (actual: unknown, expected: unknown) => expect(actual).toEqual(expected),
  true: (actual: unknown) => expect(actual).toBe(true),
  false: (actual: unknown) => expect(actual).toBe(false),
  truthy: (actual: unknown) => expect(actual).toBeTruthy(),
  regex: (actual: string, regex: RegExp) => expect(actual).toMatch(regex),
  throws,
  throwsAsync,
  notThrows: (fn: () => unknown) => expect(fn).not.toThrow(),
  fail: (message?: string): never => {
    throw new Error(message ?? 'Test failed')
  },
  pass: () => expect(true).toBe(true),
}

export function throws<T extends Error = Error>(fn: () => unknown, options?: ThrowsOptions): T {
  try {
    fn()
  } catch (error) {
    checkError(error, options)
    return error as T
  }
  throw new Error('Function did not throw')
}

export async function throwsAsync<T extends Error = Error>(
  value: (() => unknown | Promise<unknown>) | Promise<unknown>,
  options?: ThrowsOptions,
): Promise<T> {
  try {
    await (typeof value === 'function' ? value() : value)
  } catch (error) {
    checkError(error, options)
    return error as T
  }
  throw new Error('Function did not throw')
}

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
