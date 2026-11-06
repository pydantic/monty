// In-process gRPC servers for the `MontyRemote` specs, the Node counterparts
// of the fixtures in crates/monty-python/tests/test_remote.py. Each session
// stream is bridged to a real `monty subprocess` child: the child
// length-prefixes each protocol frame on stdio, gRPC carries one frame per
// message, so the bridge only adds and strips the prefix. The service is
// declared by hand over raw bytes — no schema, nothing generated.

import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process'

import {
  Metadata,
  Server,
  ServerCredentials,
  type ServerDuplexStream,
  type ServiceDefinition,
  status,
} from '@grpc/grpc-js'
import { findMontyBinary } from '@pydantic/monty/node'

/** One session stream, from the server's side. */
export type Stream = ServerDuplexStream<Buffer, Buffer>

/** A listening test server. */
export interface TestServer {
  url: string
  /** The request metadata of every stream opened, in arrival order. */
  headers: Record<string, string>[]
  close(): Promise<void>
}

/** The one RPC the pool calls, from `crates/monty-proto/proto/monty/v1/monty.proto`. */
const SESSION_METHOD = '/monty.v1.MontySession/Session'

/** `MAX_FRAME_LEN` in `monty-proto`, plus a little for gRPC's own framing. */
const MAX_MESSAGE_BYTES = 256 * 1024 * 1024 + 1024

const identity = (buffer: Buffer): Buffer => buffer

/** The session RPC over raw bytes: identity serializers in both directions. */
const sessionService: ServiceDefinition = {
  Session: {
    path: SESSION_METHOD,
    requestStream: true,
    responseStream: true,
    requestSerialize: identity,
    requestDeserialize: identity,
    responseSerialize: identity,
    responseDeserialize: identity,
  },
}

/** Serves `handle` for each session stream on an ephemeral loopback port. */
export async function serve(handle: (stream: Stream) => Promise<void> | void): Promise<TestServer> {
  const server = new Server({
    'grpc.max_receive_message_length': MAX_MESSAGE_BYTES,
    'grpc.max_send_message_length': MAX_MESSAGE_BYTES,
  })
  const headers: Record<string, string>[] = []
  server.addService(sessionService, {
    Session: (stream: Stream) => {
      headers.push(asciiMetadata(stream.metadata))
      void Promise.resolve(handle(stream)).catch((error: unknown) => stream.destroy(error as Error))
    },
  })
  const port = await new Promise<number>((resolve, reject) => {
    server.bindAsync('127.0.0.1:0', ServerCredentials.createInsecure(), (error, bound) =>
      error ? reject(error) : resolve(bound),
    )
  })
  return {
    url: `http://127.0.0.1:${port}`,
    headers,
    close: () =>
      new Promise<void>((resolve) => {
        server.forceShutdown()
        resolve()
      }),
  }
}

/** The ASCII entries of a stream's request metadata, as a plain record. */
function asciiMetadata(metadata: Metadata): Record<string, string> {
  const record: Record<string, string> = {}
  for (const [key, values] of Object.entries(metadata.getMap())) {
    if (typeof values === 'string') record[key] = values
  }
  return record
}

/**
 * Sends the response headers that admit the stream. The pool sends nothing,
 * not even `Configure`, until it sees them, so a handler that waits for a
 * request first deadlocks with it; a refusal instead ends the stream *before*
 * this, as a trailers-only status the pool reports as a failed checkout.
 */
export function admit(stream: Stream): void {
  stream.sendMetadata(new Metadata())
}

/** Ends a stream with `code` and `details`, naming `cause` in the `monty-cause` trailer when given. */
export function endWithStatus(stream: Stream, code: status, details: string, cause?: string): void {
  const metadata = new Metadata()
  if (cause !== undefined) metadata.set('monty-cause', cause)
  stream.emit('error', { code, details, metadata })
}

/** The plain relay, as `scripts/grpc_relay.py`: frames pass through unchanged. */
export function relay(stream: Stream): void {
  admit(stream)
  const child = new ChildFrames()
  stream.on('data', (data: Buffer) => child.send(data))
  stream.on('end', () => child.endInput())
  stream.on('close', () => child.kill())
  void (async () => {
    for (;;) {
      const frame = await child.next()
      if (frame === null) break
      stream.write(frame)
    }
    stream.end()
  })()
}

/** A relay that drops the stream, with an `UNAVAILABLE` status, on its `dropAt`-th request. */
export function droppingRelay(dropAt: number): (stream: Stream) => Promise<void> {
  return async (stream) => {
    admit(stream)
    const child = new ChildFrames()
    let requests = 0
    try {
      for await (const message of messages(stream)) {
        requests += 1
        if (requests === dropAt) {
          endWithStatus(stream, status.UNAVAILABLE, 'relay dropped the session')
          return
        }
        child.send(message)
        await forwardReplies(child, stream)
      }
      stream.end()
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
const EVENT_OK = 10
const EVENT_SHUTDOWN = 12
const EVENT_PING = 13
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

/** The `Ok` acknowledgement event. */
export const OK_EVENT = bytesField(EVENT_OK, Buffer.alloc(0))

export const CPU_CLOCK_OK_EVENT = Buffer.concat([OK_EVENT, Buffer.from([0xf0, 0x01, 0x01])])

/** The liveness probe a serving relay sends between turns. */
export const PING_EVENT = bytesField(EVENT_PING, Buffer.alloc(0))

/**
 * A server that answers the session's `Configure` with a `RuntimeError`
 * naming `message`, as a server refusing a profile does, then ends the stream.
 */
export function refusingServer(message: string): (stream: Stream) => Promise<void> {
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
export function drainingServer(stream: Stream): Promise<void> {
  return answerConfigure(bytesField(EVENT_SHUTDOWN, Buffer.alloc(0)))(stream)
}

/** Answers the first request with `reply` instead of running it, then ends the stream. */
function answerConfigure(reply: Buffer): (stream: Stream) => Promise<void> {
  return async (stream) => {
    admit(stream)
    for await (const _ of messages(stream)) {
      stream.write(reply)
      stream.end()
      return
    }
  }
}

/**
 * A fake of a server that stores sessions, as `_StoringServer` in the Python
 * tests. It names the session on the reply to `Configure` and to `Load`, and
 * does not run the third request of the first stream: the child is dumped,
 * its state kept under the session's ID, and the request answered with
 * `ShutdownDump` naming that ID. A later `Load` of the ID gets the stored state.
 */
export class StoringServer {
  private readonly records = new Map<string, Buffer>()
  private minted = 0
  private streams = 0

  readonly handle = async (stream: Stream): Promise<void> => {
    admit(stream)
    this.streams += 1
    // request 1 is `Configure`, 2 the first feed; the first stream parks the next
    const drainAt = this.streams === 1 ? 3 : null
    const child = new ChildFrames()
    let sessionId: Buffer | null = null
    let requests = 0
    try {
      for await (let message of messages(stream)) {
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
          stream.write(bytesField(EVENT_SHUTDOWN, bytesField(SHUTDOWN_DUMP_DUMP, sessionId!)))
          stream.end()
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
          stream.write(reply)
          if (kind !== EVENT_PRINT) break
        }
      }
      stream.end()
    } finally {
      child.kill()
    }
  }
}

/** Sends the child's replies to `stream` up to and including the turn-ending one. */
async function forwardReplies(child: ChildFrames, stream: Stream): Promise<void> {
  for (;;) {
    const reply = await child.next()
    if (reply === null) return
    stream.write(reply)
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

  /** Closes the child's stdin: it reads EOF as a clean end and exits. */
  endInput(): void {
    this.child.stdin.end()
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

/** The stream's request messages, ending when the pool half-closes or cancels it. */
export async function* messages(stream: Stream): AsyncGenerator<Buffer> {
  const queue: Buffer[] = []
  let closed = false
  let wake: (() => void) | null = null
  stream.on('data', (data: Buffer) => {
    queue.push(data)
    wake?.()
  })
  const finish = () => {
    closed = true
    wake?.()
  }
  stream.on('end', finish)
  stream.on('close', finish)
  stream.on('error', finish)
  for (;;) {
    while (queue.length === 0 && !closed) {
      await new Promise<void>((resolve) => (wake = resolve))
    }
    const next = queue.shift()
    if (next === undefined) return
    yield next
  }
}

/**
 * The top-level fields of one protobuf message, keyed by field number: the
 * bytes of a length-delimited field, and other wire types as their raw bytes.
 * Only what the fakes need, so a repeated field keeps its last value.
 */
export function protoFields(buf: Buffer): Map<number, Buffer> {
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
