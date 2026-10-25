// In-process WebSocket servers for the `MontyWebsocket` specs, the Node
// counterparts of the fixtures in crates/monty-python/tests/test_websocket.py.
// Each connection is bridged to a real `monty subprocess` child: the child
// length-prefixes each protocol frame on stdio, the WebSocket carries one
// frame per binary message, so the bridge only adds and strips the prefix.

import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process'
import type { IncomingHttpHeaders } from 'node:http'
import type { AddressInfo } from 'node:net'

import { findMontyBinary } from '@pydantic/monty/node'
import { type RawData, type WebSocket, WebSocketServer } from 'ws'

/** A listening test server. */
export interface TestServer {
  url: string
  /** The headers of every upgrade request received, in arrival order. */
  headers: IncomingHttpHeaders[]
  close(): Promise<void>
}

/** Serves `handle` for each connection on an ephemeral loopback port. */
export async function serve(handle: (ws: WebSocket) => Promise<void> | void): Promise<TestServer> {
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0, maxPayload: 0 })
  const headers: IncomingHttpHeaders[] = []
  server.on('connection', (ws, request) => {
    headers.push(request.headers)
    void Promise.resolve(handle(ws)).catch(() => ws.terminate())
  })
  await new Promise<void>((resolve) => server.once('listening', resolve))
  const { port } = server.address() as AddressInfo
  return {
    url: `ws://127.0.0.1:${port}`,
    headers,
    close: () =>
      new Promise<void>((resolve) => {
        for (const client of server.clients) client.terminate()
        server.close(() => resolve())
      }),
  }
}

/** The plain relay, as `scripts/websocket_relay.py`: frames pass through unchanged. */
export function relay(ws: WebSocket): void {
  const child = new ChildFrames()
  ws.on('message', (data) => child.send(toBuffer(data)))
  ws.on('close', () => child.kill())
  void (async () => {
    for (;;) {
      const frame = await child.next()
      if (frame === null) break
      ws.send(frame)
    }
    ws.terminate()
  })()
}

/** A relay that drops the connection, without a reply, on its `dropAt`-th request. */
export function droppingRelay(dropAt: number): (ws: WebSocket) => Promise<void> {
  return async (ws) => {
    const child = new ChildFrames()
    let requests = 0
    try {
      for await (const message of messages(ws)) {
        requests += 1
        if (requests === dropAt) {
          ws.terminate()
          return
        }
        child.send(message)
        await forwardReplies(child, ws)
      }
    } finally {
      child.kill()
    }
  }
}

// Field numbers from `crates/monty-proto/proto/monty/v1/monty.proto`.
// `ParentRequest.kind` arms:
const REQUEST_DUMP = 7
const REQUEST_LOAD = 8
// `ChildEvent.kind` arms, which own the field numbers below `FIRST_EVENT_FIELD`:
const EVENT_PRINT = 1
const EVENT_ERROR = 7
const EVENT_DUMP_RESULT = 9
const EVENT_SHUTDOWN = 12
// `ChildEvent`'s message-level fields:
const FIRST_EVENT_FIELD = 20
const EVENT_SESSION_ID = 28
// Fields of the messages those arms carry:
const LOAD_STATE = 1
const DUMP_RESULT_STATE = 1
const SHUTDOWN_DUMP_DUMP = 1
const ERROR_EXCEPTION = 1
const RAISED_EXCEPTION_EXC_TYPE = 1
const RAISED_EXCEPTION_MESSAGE = 2

// Protobuf's encoding (https://protobuf.dev/programming-guides/encoding/):
// a field's tag is a varint holding `field_number << 3 | wire_type`.
const TAG_WIRE_TYPE_BITS = 3
const TAG_WIRE_TYPE_MASK = 0b111
const WIRE_VARINT = 0
const WIRE_FIXED64 = 1
const WIRE_LENGTH_DELIMITED = 2
const WIRE_FIXED32 = 5
// A varint carries 7 bits per byte, low group first; the high bit means "more bytes follow".
const VARINT_PAYLOAD_BITS = 7
const VARINT_PAYLOAD_MASK = 0x7f
const VARINT_CONTINUES = 0x80

/**
 * A server that answers the session's `Configure` with a `RuntimeError`
 * naming `message`, as a server refusing a profile does, then hangs up.
 */
export function refusingServer(message: string): (ws: WebSocket) => Promise<void> {
  return answerConfigure(
    bytesField(
      EVENT_ERROR,
      bytesField(
        ERROR_EXCEPTION,
        Buffer.concat([
          bytesField(RAISED_EXCEPTION_EXC_TYPE, Buffer.from('RuntimeError')),
          bytesField(RAISED_EXCEPTION_MESSAGE, Buffer.from(message)),
        ]),
      ),
    ),
  )
}

/** A draining server: answers `Configure` with a shutdown naming no stored session. */
export function drainingServer(ws: WebSocket): Promise<void> {
  return answerConfigure(bytesField(EVENT_SHUTDOWN, Buffer.alloc(0)))(ws)
}

/** Answers the first request with `reply` instead of running it, then closes. */
function answerConfigure(reply: Buffer): (ws: WebSocket) => Promise<void> {
  return async (ws) => {
    for await (const _ of messages(ws)) {
      ws.send(reply)
      ws.close()
      return
    }
  }
}

/**
 * A fake of a server that stores sessions, as `_StoringServer` in the Python
 * tests. It names the session on the reply to `Configure` and to `Load`, and
 * does not run the third request of the first connection: the child is dumped,
 * its state kept under the session's ID, and the request answered with
 * `ShutdownDump` naming that ID. A later `Load` of the ID gets the stored state.
 */
export class StoringServer {
  private readonly records = new Map<string, Buffer>()
  private minted = 0
  private connections = 0

  readonly handle = async (ws: WebSocket): Promise<void> => {
    this.connections += 1
    // request 1 is `Configure`, 2 the first feed; the first connection parks the next
    const drainAt = this.connections === 1 ? 3 : null
    const child = new ChildFrames()
    let sessionId: Buffer | null = null
    let requests = 0
    try {
      for await (let message of messages(ws)) {
        requests += 1
        const fields = protoFields(message)
        if (requests === drainAt) {
          // park: dump the idle child, keep its state under the ID, and tell
          // the client the request did not run
          child.send(bytesField(REQUEST_DUMP, Buffer.alloc(0)))
          const reply = protoFields((await child.next())!)
          this.records.set(
            sessionId!.toString('latin1'),
            protoFields(reply.get(EVENT_DUMP_RESULT)!).get(DUMP_RESULT_STATE)!,
          )
          ws.send(bytesField(EVENT_SHUTDOWN, bytesField(SHUTDOWN_DUMP_DUMP, sessionId!)))
          ws.close()
          return
        }
        const names = requests === 1 || fields.has(REQUEST_LOAD)
        if (fields.has(REQUEST_LOAD)) {
          // the child loads the stored bytes the ID stands for
          const id = protoFields(fields.get(REQUEST_LOAD)!).get(LOAD_STATE)!
          message = bytesField(REQUEST_LOAD, bytesField(LOAD_STATE, this.records.get(id.toString('latin1'))!))
        }
        child.send(message)
        for (;;) {
          let reply = (await child.next())!
          const kind = Math.min(...[...protoFields(reply).keys()].filter((field) => field < FIRST_EVENT_FIELD))
          if (kind !== EVENT_PRINT && names) {
            this.minted += 1
            sessionId = Buffer.from(`sess-${this.minted}`)
            reply = Buffer.concat([reply, bytesField(EVENT_SESSION_ID, sessionId)])
          }
          ws.send(reply)
          if (kind !== EVENT_PRINT) break
        }
      }
    } finally {
      child.kill()
    }
  }
}

/** Sends the child's replies to `ws` up to and including the turn-ending one. */
async function forwardReplies(child: ChildFrames, ws: WebSocket): Promise<void> {
  for (;;) {
    const reply = await child.next()
    if (reply === null) return
    ws.send(reply)
    const kind = Math.min(...[...protoFields(reply).keys()].filter((field) => field < FIRST_EVENT_FIELD))
    if (kind !== EVENT_PRINT) return
  }
}

/** One `monty subprocess` child, framed: whole frames in, whole frames out. */
class ChildFrames {
  private readonly child: ChildProcessWithoutNullStreams
  private buffered = Buffer.alloc(0)
  private readonly frames: Buffer[] = []
  private ended = false
  private wake: (() => void) | null = null

  constructor() {
    this.child = spawn(findMontyBinary(), ['subprocess'])
    this.child.stdin.on('error', () => {})
    this.child.stdout.on('data', (chunk: Buffer) => {
      this.buffered = Buffer.concat([this.buffered, chunk])
      while (this.buffered.length >= 4 && this.buffered.length >= 4 + this.buffered.readUInt32LE(0)) {
        const length = this.buffered.readUInt32LE(0)
        this.frames.push(this.buffered.subarray(4, 4 + length))
        this.buffered = this.buffered.subarray(4 + length)
      }
      this.notify()
    })
    this.child.stdout.on('end', () => {
      this.ended = true
      this.notify()
    })
  }

  send(frame: Buffer): void {
    const prefix = Buffer.alloc(4)
    prefix.writeUInt32LE(frame.length)
    this.child.stdin.write(Buffer.concat([prefix, frame]))
  }

  /** The next frame from the child, or `null` once its output ends. */
  async next(): Promise<Buffer | null> {
    while (this.frames.length === 0 && !this.ended) {
      await new Promise<void>((resolve) => (this.wake = resolve))
    }
    return this.frames.shift() ?? null
  }

  kill(): void {
    this.child.kill()
  }

  private notify(): void {
    this.wake?.()
    this.wake = null
  }
}

/** The connection's binary messages, ending when it closes. */
async function* messages(ws: WebSocket): AsyncGenerator<Buffer> {
  const queue: Buffer[] = []
  let closed = false
  let wake: (() => void) | null = null
  ws.on('message', (data) => {
    queue.push(toBuffer(data))
    wake?.()
  })
  ws.on('close', () => {
    closed = true
    wake?.()
  })
  for (;;) {
    while (queue.length === 0 && !closed) {
      await new Promise<void>((resolve) => (wake = resolve))
    }
    const next = queue.shift()
    if (next === undefined) return
    yield next
  }
}

function toBuffer(data: RawData): Buffer {
  return Array.isArray(data) ? Buffer.concat(data) : Buffer.from(data as ArrayBuffer)
}

/**
 * The top-level fields of one protobuf message, keyed by field number: the
 * bytes of a length-delimited field, and other wire types as their raw bytes.
 * Only what the fakes need, so a repeated field keeps its last value.
 */
function protoFields(buf: Buffer): Map<number, Buffer> {
  const fields = new Map<number, Buffer>()
  let i = 0
  while (i < buf.length) {
    let key: number
    ;[key, i] = varint(buf, i)
    const field = key >>> TAG_WIRE_TYPE_BITS
    const wireType = key & TAG_WIRE_TYPE_MASK
    const start = i
    if (wireType === WIRE_VARINT) {
      ;[, i] = varint(buf, i)
    } else if (wireType === WIRE_LENGTH_DELIMITED) {
      let length: number
      ;[length, i] = varint(buf, i)
      fields.set(field, buf.subarray(i, i + length))
      i += length
      continue
    } else if (wireType === WIRE_FIXED64) {
      i += 8
    } else if (wireType === WIRE_FIXED32) {
      i += 4
    } else {
      throw new Error(`unsupported wire type ${wireType}`)
    }
    fields.set(field, buf.subarray(start, i))
  }
  return fields
}

/** Decodes the varint at `buf[i]`, returning its value and the index after it. */
function varint(buf: Buffer, i: number): [number, number] {
  let value = 0
  let shift = 0
  for (;;) {
    const byte = buf[i++]!
    value += (byte & VARINT_PAYLOAD_MASK) * 2 ** shift
    if ((byte & VARINT_CONTINUES) === 0) return [value, i]
    shift += VARINT_PAYLOAD_BITS
  }
}

/** Encodes a non-negative integer as a varint. */
function encodeVarint(value: number): Buffer {
  const out: number[] = []
  while (value > VARINT_PAYLOAD_MASK) {
    out.push((value & VARINT_PAYLOAD_MASK) | VARINT_CONTINUES)
    value = Math.floor(value / 2 ** VARINT_PAYLOAD_BITS)
  }
  out.push(value)
  return Buffer.from(out)
}

/** Encodes `data` as length-delimited field `field`: a bytes, string or embedded-message field. */
function bytesField(field: number, data: Buffer): Buffer {
  return Buffer.concat([
    encodeVarint((field << TAG_WIRE_TYPE_BITS) | WIRE_LENGTH_DELIMITED),
    encodeVarint(data.length),
    data,
  ])
}
