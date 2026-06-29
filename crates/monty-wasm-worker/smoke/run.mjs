// Spike smoke test for the lean wasip1 Monty worker (`crates/monty-wasm-worker`).
//
// Loads `monty_wasm_worker.wasm` under a single-threaded WASI shim in Node, drives one
// `ReplCreate` turn and one `ReplFeed` turn over WASI stdio, and checks the
// reply frames. This proves the three things the in-process Rust tests cannot:
//   1. the module instantiates and runs under a browser-style WASI shim;
//   2. `Instant::now()` and `getrandom` (ahash seeding) resolve at runtime via
//      the shim's clock/random calls rather than trapping;
//   3. a real feed executes in the sandbox and returns a decodable value.
//
// Request/response are hand-encoded protobuf — just the few fields this test
// touches — so it has no dependency on a generated codec yet. The real browser
// path will use a generated `MontyObject` codec (plan item #3).

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { File, OpenFile, WASI } from '@bjorn3/browser_wasi_shim'

// --- minimal protobuf encode (varint, length-delimited, bool) ---

function varint(n) {
  const out = []
  let v = BigInt(n)
  while (v > 0x7fn) {
    out.push(Number((v & 0x7fn) | 0x80n))
    v >>= 7n
  }
  out.push(Number(v))
  return out
}

const tag = (field, wire) => varint((field << 3) | wire)
const lenField = (field, bytes) => [...tag(field, 2), ...varint(bytes.length), ...bytes]
const boolField = (field, b) => [...tag(field, 0), ...varint(b ? 1 : 0)]
const utf8 = (s) => [...new TextEncoder().encode(s)]

function frameOf(bytes) {
  const len = bytes.length
  return Uint8Array.from([len & 0xff, (len >> 8) & 0xff, (len >> 16) & 0xff, (len >> 24) & 0xff, ...bytes])
}

// ParentRequest { repl_create = 1 { script_name = 1 } }
const replCreateFrame = frameOf(lenField(1, lenField(1, utf8('main.py'))))
// ParentRequest { feed = 3 { code = 1, skip_type_check = 4 } } (field 2 is
// InstallDependencies)
const replFeedFrame = frameOf(lenField(3, [...lenField(1, utf8('1 + 2')), ...boolField(4, true)]))

// --- minimal protobuf decode ---

function readVarint(buf, i) {
  let shift = 0n
  let result = 0n
  let b
  do {
    b = buf[i++]
    result |= BigInt(b & 0x7f) << shift
    shift += 7n
  } while (b & 0x80)
  return [result, i]
}

function* fields(buf) {
  let i = 0
  while (i < buf.length) {
    let key
    ;[key, i] = readVarint(buf, i)
    const field = Number(key >> 3n)
    const wire = Number(key & 7n)
    if (wire === 0) {
      let value
      ;[value, i] = readVarint(buf, i)
      yield { field, wire, value }
    } else if (wire === 2) {
      let len
      ;[len, i] = readVarint(buf, i)
      const n = Number(len)
      yield { field, wire, bytes: buf.subarray(i, i + n) }
      i += n
    } else {
      throw new Error(`unsupported wire type ${wire} for field ${field}`)
    }
  }
}

function* frames(buf) {
  let i = 0
  while (i + 4 <= buf.length) {
    const len = buf[i] | (buf[i + 1] << 8) | (buf[i + 2] << 16) | (buf[i + 3] << 24)
    i += 4
    yield buf.subarray(i, i + len)
    i += len
  }
}

// The single ChildEvent.kind oneof field present in a frame (1..=11); timing
// fields 12/13 are ignored.
function kindField(frameBytes) {
  let kind = null
  for (const f of fields(frameBytes)) {
    if (f.field >= 1 && f.field <= 11) kind = f
  }
  return kind
}

// Decodes Complete { value: MontyObject { int = 4 (sint64) } }.
function completeInt(frameBytes) {
  const kind = kindField(frameBytes)
  if (!kind || kind.field !== 6) throw new Error(`expected Complete (kind 6), got kind ${kind?.field}`)
  let value = null
  for (const f of fields(kind.bytes)) if (f.field === 1) value = f.bytes
  if (!value) throw new Error('Complete carries no value')
  for (const f of fields(value)) {
    if (f.field === 4 && f.wire === 0) {
      return Number((f.value >> 1n) ^ -(f.value & 1n)) // unzigzag sint64
    }
  }
  throw new Error('value is not an Int')
}

// --- WASI driving: one instance, reset stdio buffers per turn ---

const here = dirname(fileURLToPath(import.meta.url))
const wasmPath = join(here, '..', '..', '..', 'target', 'wasm32-wasip1', 'release', 'monty_wasm_worker.wasm')
const wasmBytes = readFileSync(wasmPath)

const wasi = new WASI([], [], [new OpenFile(new File([])), new OpenFile(new File([])), new OpenFile(new File([]))])
const module = await WebAssembly.compile(wasmBytes)
const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport })
wasi.initialize(instance)

function turn(requestFrame) {
  wasi.fds[0] = new OpenFile(new File([...requestFrame]))
  const outFile = new File([])
  wasi.fds[1] = new OpenFile(outFile)
  const status = instance.exports.monty_dispatch_turn()
  return { status, events: [...frames(outFile.data)] }
}

// turn 1: ReplCreate -> a single Ok (kind 10)
const create = turn(replCreateFrame)
if (create.status !== 0) throw new Error(`ReplCreate returned status ${create.status}`)
if (create.events.length !== 1 || kindField(create.events[0]).field !== 10) {
  throw new Error(`ReplCreate did not answer with a single Ok, got ${create.events.map((e) => kindField(e).field)}`)
}

// turn 2: ReplFeed "1 + 2" -> Complete(Int(3))
const feed = turn(replFeedFrame)
if (feed.status !== 0) throw new Error(`ReplFeed returned status ${feed.status}`)
const result = completeInt(feed.events.at(-1))
if (result !== 3) throw new Error(`expected 1 + 2 == 3, got ${result}`)

console.log('OK: ReplCreate -> Ok, ReplFeed("1 + 2") -> Complete(Int(3)) across a persistent wasm session')
