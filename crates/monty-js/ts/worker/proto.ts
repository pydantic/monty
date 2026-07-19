// Minimal protobuf wire codec for the wasm worker transport.
//
// The browser/wasm path speaks the same `monty.v1` protobuf as the native
// pool, but without a napi layer to convert values in Rust — so the TypeScript
// side encodes `ParentRequest`s and decodes `ChildEvent`s itself. This module
// is the low-level wire layer (varints, tags, length-delimited fields, 4-byte
// frame prefix); `value.ts` builds `MontyObject` on top of it and `transport.ts`
// builds the request/event messages.
//
// It is deliberately hand-rolled and tiny rather than generated: the worker
// only touches a handful of messages, and the wire format is proven
// byte-compatible against prost by `monty-proto`'s differential oracle. A
// generated codec (ts-proto / protobuf-es) is the eventual home; this keeps the
// spike dependency-free.

/** Protobuf wire types this codec handles. */
export const Wire = {
  Varint: 0,
  Fixed64: 1,
  LengthDelimited: 2,
  Fixed32: 5,
} as const

/** Accumulates protobuf bytes for one message. */
export class Writer {
  private readonly buf: number[] = []

  /** Finishes the message, returning its raw (unframed) bytes. */
  finish(): Uint8Array {
    return Uint8Array.from(this.buf)
  }

  varint(value: number | bigint): void {
    let v = BigInt(value)
    if (v < 0n) throw new Error(`varint cannot encode negative ${v}`)
    while (v > 0x7fn) {
      this.buf.push(Number((v & 0x7fn) | 0x80n))
      v >>= 7n
    }
    this.buf.push(Number(v))
  }

  private tag(field: number, wire: number): void {
    this.varint((field << 3) | wire)
  }

  uint(field: number, value: number | bigint): void {
    this.tag(field, Wire.Varint)
    this.varint(value)
  }

  /** Zig-zag encoded signed 64-bit int (proto `sint64`). */
  sint64(field: number, value: bigint): void {
    this.tag(field, Wire.Varint)
    this.varint((value << 1n) ^ (value >> 63n))
  }

  bool(field: number, value: boolean): void {
    this.tag(field, Wire.Varint)
    this.varint(value ? 1 : 0)
  }

  /** Signed 32-bit int (proto `int32`: negatives sign-extend to a 64-bit varint). */
  int32(field: number, value: number): void {
    this.tag(field, Wire.Varint)
    this.varint(BigInt.asUintN(64, BigInt(value)))
  }

  double(field: number, value: number): void {
    this.tag(field, Wire.Fixed64)
    const view = new DataView(new ArrayBuffer(8))
    view.setFloat64(0, value, true)
    for (let i = 0; i < 8; i++) this.buf.push(view.getUint8(i))
  }

  string(field: number, value: string): void {
    this.lengthDelimited(field, new TextEncoder().encode(value))
  }

  bytes(field: number, value: Uint8Array): void {
    this.lengthDelimited(field, value)
  }

  /** A nested message (or any length-prefixed byte payload) as one field. */
  lengthDelimited(field: number, payload: Uint8Array): void {
    this.tag(field, Wire.LengthDelimited)
    this.varint(payload.length)
    for (const b of payload) this.buf.push(b)
  }
}

/** One field read from a message: its number, wire type, and payload. */
export interface Field {
  readonly field: number
  readonly wire: number
  /** Raw varint / fixed value (Varint, Fixed64, Fixed32). */
  readonly value: bigint
  /** Payload bytes (LengthDelimited only). */
  readonly bytes: Uint8Array
}

/** Reads protobuf fields out of one message's bytes. */
export class Reader {
  private pos: number

  constructor(
    private readonly buf: Uint8Array,
    start = 0,
    private readonly end = buf.length,
  ) {
    this.pos = start
  }

  get done(): boolean {
    return this.pos >= this.end
  }

  nextVarint(): bigint {
    return this.varint()
  }

  private varint(): bigint {
    let shift = 0n
    let result = 0n
    for (;;) {
      const b = this.buf[this.pos++]
      result |= BigInt(b & 0x7f) << shift
      if ((b & 0x80) === 0) break
      shift += 7n
    }
    return result
  }

  /** Reads the next field; call only while `!done`. */
  next(): Field {
    const key = this.varint()
    const field = Number(key >> 3n)
    const wire = Number(key & 7n)
    switch (wire) {
      case Wire.Varint:
        return { field, wire, value: this.varint(), bytes: EMPTY }
      case Wire.Fixed64: {
        const value = this.fixed(8)
        return { field, wire, value, bytes: EMPTY }
      }
      case Wire.Fixed32: {
        const value = this.fixed(4)
        return { field, wire, value, bytes: EMPTY }
      }
      case Wire.LengthDelimited: {
        const len = Number(this.varint())
        const bytes = this.buf.subarray(this.pos, this.pos + len)
        this.pos += len
        return { field, wire, value: 0n, bytes }
      }
      default:
        throw new Error(`unsupported wire type ${wire} for field ${field}`)
    }
  }

  private fixed(n: number): bigint {
    let value = 0n
    for (let i = 0; i < n; i++) value |= BigInt(this.buf[this.pos++]) << BigInt(8 * i)
    return value
  }
}

const EMPTY = new Uint8Array(0)

/** Decodes an IEEE-754 double from a `Fixed64` field's raw bits. */
export function bitsToDouble(bits: bigint): number {
  const view = new DataView(new ArrayBuffer(8))
  view.setBigUint64(0, bits, true)
  return view.getFloat64(0, true)
}

/** Un–zig-zags a `sint64` field's raw varint into a signed BigInt. */
export function unzigzag(value: bigint): bigint {
  return (value >> 1n) ^ -(value & 1n)
}

/** Reads a proto `int32` field's raw varint as a signed JS number. */
export function readInt32(value: bigint): number {
  return Number(BigInt.asIntN(64, value))
}

/** Prepends the 4-byte little-endian length prefix that frames one message. */
export function frame(message: Uint8Array): Uint8Array {
  const len = message.length
  const out = new Uint8Array(4 + len)
  out[0] = len & 0xff
  out[1] = (len >> 8) & 0xff
  out[2] = (len >> 16) & 0xff
  out[3] = (len >> 24) & 0xff
  out.set(message, 4)
  return out
}

/** Splits a buffer of concatenated 4-byte-prefixed frames into messages. */
export function* deframe(buf: Uint8Array): Generator<Uint8Array> {
  let i = 0
  while (i + 4 <= buf.length) {
    // `>>> 0` keeps the length unsigned: `<< 24` yields a *signed* 32-bit
    // value, and a negative length would walk `i` backwards forever.
    const len = (buf[i] | (buf[i + 1] << 8) | (buf[i + 2] << 16) | (buf[i + 3] << 24)) >>> 0
    i += 4
    yield buf.subarray(i, i + len)
    i += len
  }
}
