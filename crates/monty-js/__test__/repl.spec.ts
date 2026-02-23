import test from 'ava'

import { MontyRepl } from '../wrapper'

test('create and feed preserve state without replay', (t) => {
  const repl = MontyRepl.create('counter = 0')

  t.true(repl instanceof MontyRepl)
  t.is(repl.feed('counter = counter + 1'), null)
  t.is(repl.feed('counter'), 1)
  t.is(repl.feed('counter = counter + 1'), null)
  t.is(repl.feed('counter'), 2)
})

test('create accepts start inputs', (t) => {
  const repl = MontyRepl.create('counter = start', { inputs: ['start'] }, { inputs: { start: 3 } })

  t.is(repl.feed('counter'), 3)
  t.is(repl.feed('counter = counter + 2'), null)
  t.is(repl.feed('counter'), 5)
})

test('repl dump/load roundtrip', (t) => {
  const repl = MontyRepl.create('x = 40')
  t.is(repl.feed('x = x + 1'), null)

  const serialized = repl.dump()
  const loaded = MontyRepl.load(serialized)

  t.is(loaded.feed('x + 1'), 42)
})
