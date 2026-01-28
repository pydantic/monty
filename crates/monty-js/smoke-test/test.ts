import { Monty, MontySyntaxError, MontyRuntimeError, MontySnapshot, MontyComplete } from '@pydantic/monty'

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

function assertThrows<T extends Error>(fn: () => void, errorClass: new (...args: never[]) => T, message: string): void {
  try {
    fn()
    console.error(`FAIL: ${message} - no error thrown`)
    failed++
  } catch (e) {
    if (e instanceof errorClass) {
      console.log(`PASS: ${message}`)
      passed++
    } else {
      console.error(`FAIL: ${message} - wrong error type: ${(e as Error).constructor.name}`)
      failed++
    }
  }
}

console.log('=== Basic Execution ===')

const m1 = new Monty('1 + 2')
assert(m1.run() === 3, 'basic arithmetic')

const m2 = new Monty('10 * 5 - 3')
assert(m2.run() === 47, 'complex arithmetic')

const m3 = new Monty('"hello" + " " + "world"')
assert(m3.run() === 'hello world', 'string concatenation')

console.log('\n=== Constructor Options ===')

const m4 = new Monty('x + y', { inputs: ['x', 'y'] })
assert(m4.inputs.length === 2, 'inputs array populated')
assert(m4.inputs[0] === 'x', 'first input correct')

const m5 = new Monty('foo()', { externalFunctions: ['foo'] })
assert(m5.externalFunctions.length === 1, 'external functions array populated')
assert(m5.externalFunctions[0] === 'foo', 'external function name correct')

const m6 = new Monty('1', { scriptName: 'custom.py' })
assert(m6.scriptName === 'custom.py', 'custom script name')

console.log('\n=== Inputs ===')

const m7 = new Monty('x * 2', { inputs: ['x'] })
assert(m7.run({ inputs: { x: 5 } }) === 10, 'single input')
assert(m7.run({ inputs: { x: -3 } }) === -6, 'negative input')

const m8 = new Monty('a + b + c', { inputs: ['a', 'b', 'c'] })
assert(m8.run({ inputs: { a: 1, b: 2, c: 3 } }) === 6, 'multiple inputs')

console.log('\n=== Error Handling ===')

assertThrows(() => new Monty('def'), MontySyntaxError, 'syntax error throws MontySyntaxError')

assertThrows(() => new Monty('1/0').run(), MontyRuntimeError, 'division by zero throws MontyRuntimeError')

assertThrows(
  () => new Monty('raise ValueError("test")').run(),
  MontyRuntimeError,
  'raise statement throws MontyRuntimeError',
)

console.log('\n=== Error Properties ===')

try {
  new Monty('raise ValueError("custom message")').run()
} catch (e) {
  if (e instanceof MontyRuntimeError) {
    assert(e.exception.typeName === 'ValueError', 'exception typeName correct')
    assert(e.exception.message === 'custom message', 'exception message correct')
    assert(e.display('msg') === 'custom message', 'display msg format')
    assert(e.display('type-msg') === 'ValueError: custom message', 'display type-msg format')
    const frames = e.traceback()
    assert(Array.isArray(frames), 'traceback returns array')
  }
}

console.log('\n=== External Functions (start/resume) ===')

const m9 = new Monty('foo(42)', { externalFunctions: ['foo'] })
const result9 = m9.start()
assert(result9 instanceof MontySnapshot, 'start returns MontySnapshot')
if (!(result9 instanceof MontySnapshot)) throw new Error('Expected MontySnapshot')
assert(result9.functionName === 'foo', 'snapshot has correct function name')
assert(result9.args[0] === 42, 'snapshot has correct args')
assert(Object.keys(result9.kwargs).length === 0, 'snapshot has empty kwargs')

const complete1 = result9.resume({ returnValue: 'result' })
assert(complete1 instanceof MontyComplete, 'resume returns MontyComplete')
if (!(complete1 instanceof MontyComplete)) throw new Error('Expected MontyComplete')
assert(complete1.output === 'result', 'complete has correct output')

console.log('\n=== External Functions with kwargs ===')

const m10 = new Monty('bar(1, 2, x=3, y=4)', { externalFunctions: ['bar'] })
const result10 = m10.start()
if (!(result10 instanceof MontySnapshot)) throw new Error('Expected MontySnapshot')
assert(result10.args[0] === 1, 'positional arg 1')
assert(result10.args[1] === 2, 'positional arg 2')
assert(result10.kwargs['x'] === 3, 'kwarg x')
assert(result10.kwargs['y'] === 4, 'kwarg y')
result10.resume({ returnValue: null })

console.log('\n=== Multiple External Calls ===')

const m11 = new Monty('a = get_a()\nb = get_b()\na + b', { externalFunctions: ['get_a', 'get_b'] })
let state: MontySnapshot | MontyComplete = m11.start()

assert(state instanceof MontySnapshot, 'first call returns snapshot')
assert((state as MontySnapshot).functionName === 'get_a', 'first function is get_a')
state = (state as MontySnapshot).resume({ returnValue: 10 })

assert(state instanceof MontySnapshot, 'second call returns snapshot')
assert((state as MontySnapshot).functionName === 'get_b', 'second function is get_b')
state = (state as MontySnapshot).resume({ returnValue: 20 })

assert(state instanceof MontyComplete, 'final state is complete')
assert((state as MontyComplete).output === 30, 'result is sum of external returns')

console.log('\n=== Serialization ===')

const m12 = new Monty('x + 1', { inputs: ['x'] })
const dumped = m12.dump()
assert(dumped instanceof Buffer, 'dump returns Buffer')
assert(dumped.length > 0, 'dump is not empty')

const loaded = Monty.load(dumped)
assert(loaded.run({ inputs: { x: 10 } }) === 11, 'loaded instance works')

console.log('\n=== Snapshot Serialization ===')

const m13 = new Monty('ext(x) + 1', { inputs: ['x'], externalFunctions: ['ext'] })
const snap = m13.start({ inputs: { x: 5 } }) as MontySnapshot
const snapDumped = snap.dump()
assert(snapDumped instanceof Buffer, 'snapshot dump returns Buffer')

const snapLoaded = MontySnapshot.load(snapDumped)
assert(snapLoaded.functionName === 'ext', 'loaded snapshot has function name')
assert(snapLoaded.args[0] === 5, 'loaded snapshot has args')

const finalResult = snapLoaded.resume({ returnValue: 100 }) as MontyComplete
assert(finalResult.output === 101, 'resumed loaded snapshot works')

console.log('\n=== repr() ===')

const m14 = new Monty('1 + 1')
const repr = m14.repr()
assert(typeof repr === 'string', 'repr returns string')
assert(repr.includes('Monty'), 'repr contains Monty')

console.log('\n=== Summary ===')
console.log(`Passed: ${passed}`)
console.log(`Failed: ${failed}`)

if (failed > 0) {
  process.exit(1)
}

console.log('\nAll smoke tests passed!')
