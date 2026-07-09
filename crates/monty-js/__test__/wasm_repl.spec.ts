import { test } from 'vitest'
import { t } from './assertions.js'

import { MontyRepl } from '../ts/wasm.js'

test('feed preserves state without replay', () => {
  const repl = new MontyRepl()

  repl.feed('counter = 0')
  t.is(repl.feed('counter = counter + 1'), null)
  t.is(repl.feed('counter'), 1)
  t.is(repl.feed('counter = counter + 1'), null)
  t.is(repl.feed('counter'), 2)
})

test('constructor accepts scriptName option', () => {
  const repl = new MontyRepl({ scriptName: 'test.py' })
  t.is(repl.scriptName, 'test.py')
})

test('default scriptName is main.py', () => {
  const repl = new MontyRepl()
  t.is(repl.scriptName, 'main.py')
})

test('repl dump/load roundtrip', () => {
  const repl = new MontyRepl()
  repl.feed('x = 40')
  t.is(repl.feed('x = x + 1'), null)

  const serialized = repl.dump()
  const loaded = MontyRepl.load(serialized)

  t.is(loaded.feed('x + 1'), 42)
})
