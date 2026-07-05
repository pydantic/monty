import { Monty, MontyRuntimeError, MontySyntaxError } from '@pydantic/monty'

let passed = 0
let failed = 0

function assert(condition: boolean, message: string): void {
  if (!condition) {
    console.error(`FAIL: ${message}`)
    failed++
  } else {
    console.log(`PASS: ${message}`)
    passed++
  }
}

async function assertThrowsAsync<T extends Error>(
  fn: () => Promise<unknown>,
  errorClass: new (...args: never[]) => T,
  message: string,
): Promise<T | null> {
  try {
    await fn()
    console.error(`FAIL: ${message} - no error thrown`)
    failed++
    return null
  } catch (e) {
    if (e instanceof errorClass) {
      console.log(`PASS: ${message}`)
      passed++
      return e
    }
    console.error(`FAIL: ${message} - wrong error type: ${(e as Error).constructor.name}: ${(e as Error).message}`)
    failed++
    return null
  }
}

console.log('=== Pool and session lifecycle ===')

const pool = await Monty.create()
const session = await pool.checkout()
assert(typeof session.workerPid === 'number', 'session has a worker pid')

console.log('\n=== Basic Execution ===')

assert((await session.feedRun('1 + 2')) === 3, 'basic arithmetic')
assert((await session.feedRun('10 * 5 - 3')) === 47, 'complex arithmetic')
assert((await session.feedRun('"hello" + " " + "world"')) === 'hello world', 'string concatenation')

console.log('\n=== Session state ===')

await session.feedRun('x = 21')
assert((await session.feedRun('x * 2')) === 42, 'globals persist across feeds')

console.log('\n=== Inputs ===')

assert((await session.feedRun('a + b + c', { inputs: { a: 1, b: 2, c: 3 } })) === 6, 'multiple inputs')

console.log('\n=== Error Handling ===')

await assertThrowsAsync(() => session.feedRun('def'), MontySyntaxError, 'syntax error throws MontySyntaxError')
await assertThrowsAsync(() => session.feedRun('1/0'), MontyRuntimeError, 'division by zero throws MontyRuntimeError')

const err = await assertThrowsAsync(
  () => session.feedRun('raise ValueError("custom message")'),
  MontyRuntimeError,
  'raise statement throws MontyRuntimeError',
)
if (err !== null) {
  assert(err.exception.typeName === 'ValueError', 'exception typeName correct')
  assert(err.exception.message === 'custom message', 'exception message correct')
  assert(err.display('msg') === 'custom message', 'display msg format')
  assert(err.display('type-msg') === 'ValueError: custom message', 'display type-msg format')
  assert(Array.isArray(err.traceback()), 'traceback returns array')
}

console.log('\n=== External Functions ===')

assert(
  (await session.feedRun('add(2, 3)', { externalLookup: { add: (a: number, b: number) => a + b } })) === 5,
  'sync external function',
)
assert(
  (await session.feedRun('await get_data()', {
    externalLookup: { get_data: async () => 'async result' },
  })) === 'async result',
  'async external function',
)

const callArgs: unknown[] = []
await session.feedRun('bar(1, 2, x=3, y=4)', {
  externalLookup: {
    bar: (...args: unknown[]) => {
      callArgs.push(...args)
      return null
    },
  },
})
assert(JSON.stringify(callArgs) === '[1,2,{"x":3,"y":4}]', 'positional args and kwargs object')

console.log('\n=== Print callback ===')

const prints: string[] = []
await session.feedRun('print("hello")', { printCallback: (_stream, text) => prints.push(text) })
assert(JSON.stringify(prints) === '["hello\\n"]', 'print callback receives output')

console.log('\n=== Dump ===')

const dumped = await session.dump()
assert(dumped instanceof Buffer, 'dump returns Buffer')
assert(dumped.length > 0, 'dump is not empty')

console.log('\n=== Shutdown ===')

await session.close()
await pool.close()
assert(true, 'pool closed cleanly')

console.log('\n=== Summary ===')
console.log(`Passed: ${passed}`)
console.log(`Failed: ${failed}`)

if (failed > 0) {
  process.exit(1)
}

console.log('\nAll smoke tests passed!')
