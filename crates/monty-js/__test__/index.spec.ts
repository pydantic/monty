import test from 'ava'

import { run } from '../index'

test('run simple expression', (t) => {
  const result = run('1 + 2')
  t.is(result.output, '')
  t.is(result.result, 'Int(3)')
})

test('run with print output', (t) => {
  const result = run('print("hello")')
  t.is(result.output, 'hello\n')
  t.is(result.result, 'None')
})

test('run with multiple prints', (t) => {
  const result = run('print("a")\nprint("b")\n3 + 4')
  t.is(result.output, 'a\nb\n')
  t.is(result.result, 'Int(7)')
})

test('run with syntax error', (t) => {
  const error = t.throws(() => run('def'))
  t.true(error?.message.includes('SyntaxError'))
})

test('run with runtime error', (t) => {
  const error = t.throws(() => run('raise ValueError("oops")'))
  t.true(error?.message.includes('ValueError: oops'))
})

test('run with string result', (t) => {
  const result = run('"hello" + " world"')
  t.is(result.result, 'String("hello world")')
})

test('run with list result', (t) => {
  const result = run('[1, 2, 3]')
  t.is(result.result, 'List([Int(1), Int(2), Int(3)])')
})
