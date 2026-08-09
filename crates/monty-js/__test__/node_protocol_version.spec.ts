import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { expect, test } from 'vitest'
import { PROTOCOL_VERSION } from '../ts/worker/proto.js'

// `ts/worker/` is a second, hand-written implementation of the same wire
// protocol, so its version constant is a copy of the Rust one with no compiler
// tying them together. A bump on one side alone makes the wasm worker reject
// every session with `unsupported protocol version N`, which otherwise only
// shows up once `make test-wasm` runs a real session.
test('the TS codec speaks the same protocol version as monty-proto', () => {
  const libRs = readFileSync(fileURLToPath(new URL('../../monty-proto/src/lib.rs', import.meta.url)), 'utf8')
  const match = libRs.match(/pub const PROTOCOL_VERSION: u32 = (\d+);/)
  expect(match, 'PROTOCOL_VERSION not found in monty-proto/src/lib.rs — did it move or get renamed?').not.toBeNull()
  expect(Number(match![1])).toBe(PROTOCOL_VERSION)
})
