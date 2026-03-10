import test from 'ava'

import { MontyRepl } from '../wrapper'

test('feed preserves state without replay', (t) => {
  const repl = new MontyRepl()

  repl.feed('counter = 0')
  t.is(repl.feed('counter = counter + 1'), null)
  t.is(repl.feed('counter'), 1)
  t.is(repl.feed('counter = counter + 1'), null)
  t.is(repl.feed('counter'), 2)
})

test('constructor accepts scriptName option', (t) => {
  const repl = new MontyRepl({ scriptName: 'test.py' })
  t.is(repl.scriptName, 'test.py')
})

test('default scriptName is main.py', (t) => {
  const repl = new MontyRepl()
  t.is(repl.scriptName, 'main.py')
})

test('repl dump/load roundtrip', (t) => {
  const repl = new MontyRepl()
  repl.feed('x = 40')
  t.is(repl.feed('x = x + 1'), null)

  const serialized = repl.dump()
  const loaded = MontyRepl.load(serialized)

  t.is(loaded.feed('x + 1'), 42)
})
