import { expect } from 'vitest'

interface ThrowsOptions<T extends Error> {
  instanceOf?: new (...args: never[]) => T
  message?: string | RegExp
}

export const t = {
  is: (actual: unknown, expected: unknown, message?: string) => expect(actual, message).toBe(expected),
  not: (actual: unknown, expected: unknown) => expect(actual).not.toBe(expected),
  deepEqual: (actual: unknown, expected: unknown, message?: string) => expect(actual, message).toEqual(expected),
  true: (actual: unknown, message?: string) => expect(actual, message).toBe(true),
  false: (actual: unknown) => expect(actual).toBe(false),
  truthy: (actual: unknown, message?: string) => expect(actual, message).toBeTruthy(),
  regex: (actual: string, regex: RegExp) => expect(actual).toMatch(regex),
  throws,
  throwsAsync,
  notThrows: (fn: () => unknown) => expect(fn).not.toThrow(),
  fail: (message?: string): never => {
    throw new Error(message ?? 'Test failed')
  },
  pass: () => expect(true).toBe(true),
}

/**
 * Assert a `MemoryError` reports roughly `expected` bytes used against `maxMemory`.
 *
 * The figure is real allocator bytes, so the baseline a session starts from
 * drifts a few dozen bytes between platforms (macOS runs consistently below
 * Linux and Windows) — an exact match would make these tests OS-specific. The
 * tolerance stays far below what a mis-accounted allocation would move it by.
 */
export function assertMemoryError(error: Error, expected: number, maxMemory: number, tolerance = 1024): void {
  const match = /^MemoryError: memory limit exceeded: (\d+) bytes > (\d+) bytes$/.exec(error.message)
  if (match === null) {
    throw new Error(`unexpected MemoryError message: ${error.message}`)
  }
  const used = Number(match[1])
  expect(Number(match[2])).toBe(maxMemory)
  if (Math.abs(used - expected) > tolerance) {
    throw new Error(`reported ${used} bytes, expected within ${tolerance} of ${expected}`)
  }
}

export function throws<T extends Error = Error>(fn: () => unknown, options?: ThrowsOptions<T>): T {
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
  options?: ThrowsOptions<T>,
): Promise<T> {
  try {
    await (typeof value === 'function' ? value() : value)
  } catch (error) {
    checkError(error, options)
    return error as T
  }
  throw new Error('Function did not throw')
}

function checkError(error: unknown, options: ThrowsOptions<Error> | undefined): void {
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
