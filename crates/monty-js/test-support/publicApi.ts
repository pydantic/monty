import { expect, test } from 'vitest'

interface PublicMonty {
  close(): Promise<void>
  [Symbol.asyncDispose](): Promise<void>
  checkout(): Promise<PublicSession>
}

interface PublicSession {
  close(): Promise<void>
  [Symbol.asyncDispose](): Promise<void>
  feedRun(
    code: string,
    options?: { inputs?: Record<string, unknown>; printCallback?: (stream: string, text: string) => void },
  ): Promise<unknown>
}

/** Registers public API conformance tests for any `Monty.create()` backend. */
export function runMontyApiTests(name: string, create: () => Promise<PublicMonty>): void {
  test(`${name}: evaluates code and keeps session state`, async () => {
    await using pool = await create()
    await using session = await pool.checkout()

    await session.feedRun('x = 21')

    expect(await session.feedRun('x * 2')).toBe(42)
  })

  test(`${name}: accepts inputs`, async () => {
    await using pool = await create()
    await using session = await pool.checkout()

    expect(await session.feedRun('x + 1', { inputs: { x: 4 } })).toBe(5)
  })

  test(`${name}: forwards prints`, async () => {
    const printed: string[] = []
    await using pool = await create()
    await using session = await pool.checkout()

    const result = await session.feedRun("print('hello')\n123", {
      printCallback: (stream, text) => printed.push(`${stream}:${text}`),
    })

    expect(result).toBe(123)
    expect(printed).toEqual(['stdout:hello\n'])
  })
}
