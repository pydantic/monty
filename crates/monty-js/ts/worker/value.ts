// JavaScript value conversion for the semantic WASM component boundary.
//
// WIT cannot express recursive types, so values cross as the same flat
// post-order node arena the wire uses: one per message, every child index
// strictly lower than its holder's, an index used twice a shared object. This
// file maps JavaScript values to and from that arena; all protobuf encoding,
// decoding, validation, and protocol dispatch stays in Rust.

import { MontyFileHandle, canonicalFileMode, validateFilePosition } from '../types.js'
import type { Arena, NodePair, ValueNode } from './component/monty.component.js'

const I64_MIN = -(2n ** 63n)
const I64_MAX = 2n ** 63n - 1n
const SAFE = BigInt(Number.MAX_SAFE_INTEGER)
const TYPE_MARKER = '__monty_type__'

/** A non-enumerable marker stamped on arrays that came from Python tuples. */
export const TUPLE_MARKER = '__tuple__'

/** Memo entry for a container whose children are still being pushed. */
const IN_PROGRESS = -1

/** Encodes one value as its own arena: the single-value form of [`ArenaEncoder`]. */
export function encodeValue(value: unknown): { root: number; nodes: ValueNode[] } {
  const encoder = new ArenaEncoder()
  const root = encoder.push(value)
  return { root, nodes: encoder.finish().nodes }
}

/** Decodes one value from its own arena: the single-value form of [`decodeArena`]. */
export function decodeValue(value: { root: number; nodes: ValueNode[] }): unknown {
  return decodeArena({ nodes: value.nodes })(value.root)
}

/**
 * Builds one message's arena from JavaScript values, preserving sharing.
 *
 * Every container and marker object is memoized by identity for the life of
 * the encoder, so pushing the same object twice (within one value, or across
 * the inputs of a feed) yields the same node index and the sandbox sees one
 * object; primitives are re-encoded per reference. A class with no eager
 * attrs has one node per id however it is spelled. Nesting is walked on an
 * explicit stack, so depth is bounded by memory, not the call stack. A cycle
 * is rejected with `TypeError`: the arena is post-order, so a value cannot
 * reach itself.
 */
export class ArenaEncoder {
  private readonly nodes: ValueNode[] = []
  /** Object → its node index, or `IN_PROGRESS` while its children are pushed. */
  private readonly memo = new Map<object, number>()
  /** Attr-less class nodes by class uuid. */
  private readonly classTypes = new Map<string, number>()

  /** Encodes `value` into the arena and returns its node index. */
  push(value: unknown): number {
    const stack: Frame[] = []
    let next: Child = { value }
    for (;;) {
      // descend until a leaf, a memo hit, or an empty container
      let done: number
      const step = this.step(next)
      if (typeof step === 'number') {
        done = step
      } else if (step.children.length > step.next) {
        stack.push(step)
        next = step.children[step.next++]!
        continue
      } else {
        done = this.complete(step)
      }
      // hand the finished node up, completing every holder it finishes
      for (;;) {
        const frame = stack.pop()
        if (frame === undefined) {
          return done
        }
        frame.ids.push(done)
        if (frame.children.length > frame.next) {
          stack.push(frame)
          next = frame.children[frame.next++]!
          break
        }
        done = this.complete(frame)
      }
    }
  }

  /** The arena, once every root has been pushed. */
  finish(): Arena {
    return { nodes: this.nodes }
  }

  /** Resolves one pending child: a leaf is pushed at once, a container opens a frame. */
  private step(child: Child): number | Frame {
    return 'classType' in child ? this.enterClassType(child.classType) : this.encode(child.value)
  }

  private encode(value: unknown): number | Frame {
    if (value === null || value === undefined) {
      return this.leaf({ tag: 'none' })
    } else if (typeof value === 'boolean') {
      return this.leaf({ tag: 'boolean', val: value })
    } else if (typeof value === 'number') {
      return this.leaf(
        Number.isInteger(value) && (Number.isSafeInteger(value) || value === Number(I64_MIN))
          ? { tag: 'integer', val: BigInt(value) }
          : { tag: 'float', val: value },
      )
    } else if (typeof value === 'bigint') {
      return this.leaf(
        value >= I64_MIN && value <= I64_MAX
          ? { tag: 'integer', val: value }
          : { tag: 'bigint', val: value.toString() },
      )
    } else if (typeof value === 'string') {
      return this.leaf({ tag: 'text', val: value })
    } else if (value instanceof Uint8Array) {
      return this.leaf({ tag: 'bytes', val: value })
    } else if (typeof value === 'function') {
      return this.leaf({ tag: 'function', val: { name: value.name ?? '' } })
    } else if (typeof value === 'symbol') {
      throw new TypeError('Cannot convert JS Symbol to Monty value')
    } else if (typeof value !== 'object') {
      throw unsupported(`value of type ${typeof value}`)
    } else if (Array.isArray(value)) {
      return this.enter(value, { kind: isTuple(value) ? 'tuple-value' : 'list-value' }, value.map(asChild))
    } else if (value instanceof Map) {
      return this.enter(value, { kind: 'dict' }, pairChildren([...value.entries()]))
    } else if (value instanceof Set) {
      return this.enter(value, { kind: 'set' }, [...value].map(asChild))
    }
    const object = value as Record<string, unknown>
    if (TYPE_MARKER in object) {
      return this.encodeMarked(object)
    }
    return this.enter(object, { kind: 'dict' }, pairChildren(Object.entries(object)))
  }

  /** Converts a `__monty_type__` marker. */
  private encodeMarked(object: Record<string, unknown>): number | Frame {
    switch (object[TYPE_MARKER]) {
      case 'Ellipsis':
        return this.leaf({ tag: 'ellipsis' })
      case 'NotImplemented':
        return this.leaf({ tag: 'not-implemented' })
      case 'Date':
        return this.leaf({
          tag: 'date',
          val: { year: Number(object.year), month: Number(object.month), day: Number(object.day) },
        })
      case 'DateTime':
        return this.leaf({
          tag: 'datetime',
          val: {
            year: Number(object.year),
            month: Number(object.month),
            day: Number(object.day),
            hour: Number(object.hour),
            minute: Number(object.minute),
            second: Number(object.second),
            microsecond: Number(object.microsecond),
            ...timeZoneFields(object, 'DateTime'),
          },
        })
      case 'Time':
        return this.leaf({
          tag: 'time',
          val: {
            hour: Number(object.hour),
            minute: Number(object.minute),
            second: Number(object.second),
            microsecond: Number(object.microsecond),
            ...timeZoneFields(object, 'Time'),
            fold: Number(object.fold ?? 0),
          },
        })
      case 'TimeDelta':
        return this.leaf({
          tag: 'timedelta',
          val: {
            days: Number(object.days),
            seconds: Number(object.seconds),
            microseconds: Number(object.microseconds),
          },
        })
      case 'TimeZone':
        return this.leaf({
          tag: 'timezone',
          val: {
            offsetSeconds: Number(object.offsetSeconds),
            ...(object.name === undefined ? {} : { name: String(object.name) }),
          },
        })
      case 'Exception':
        return this.leaf({
          tag: 'exception',
          val: {
            excType: String(object.excType),
            ...(typeof object.message === 'string' ? { message: object.message } : {}),
          },
        })
      case 'ClassInstance':
        return this.enterClassInstance(object)
      case 'FileHandle':
        return this.leaf(fileHandleNode(object))
      case 'Type':
        // A class type marker (`classType`) crosses structurally; builtin type
        // markers carry only the name.
        if (typeof object.classType === 'object' && object.classType !== null) {
          return this.enterClassType(object.classType as Record<string, unknown>)
        }
        return this.leaf({ tag: 'type-name', val: String(object.value) })
      case 'BuiltinFunction':
        return this.leaf({ tag: 'builtin-function', val: String(object.value) })
      default:
        throw new TypeError(`Unknown Monty marker type: ${String(object[TYPE_MARKER])}`)
    }
  }

  /** Appends a leaf node. */
  private leaf(node: ValueNode): number {
    const index = this.nodes.length
    this.nodes.push(node)
    return index
  }

  /** Opens a frame for a container, unless it is memoized: a finished one
   *  returns its node, one still being pushed is a cycle. */
  private enter(object: object, pending: Pending, children: Child[]): number | Frame {
    const seen = this.memo.get(object)
    if (seen === IN_PROGRESS) {
      throw new TypeError('Circular reference detected')
    }
    if (seen !== undefined) {
      return seen
    }
    this.memo.set(object, IN_PROGRESS)
    return { pending, key: object, children, next: 0, ids: [] }
  }

  /**
   * Validates a host `ClassInstance` marker (same shape the napi path
   * produces: `attrs` as ordered `[name, value]` pairs, uuids as strings) and
   * opens its frame: the class first, then the attrs. Validation messages
   * mirror napi's so both transports fail malformed markers alike.
   */
  private enterClassInstance(object: Record<string, unknown>): number | Frame {
    if (typeof object.type !== 'object' || object.type === null) {
      throw new TypeError(
        `Object property 'type' type mismatch. Expect value to be Object, but received ${jsType(object.type)}`,
      )
    }
    if (!Array.isArray(object.attrs)) {
      throw new TypeError(
        `Object property 'attrs' type mismatch. Expect value to be Array, but received ${jsType(object.attrs)}`,
      )
    }
    const instanceId = uuidString(object.instanceId, 'ClassInstance instanceId')
    const children: Child[] = [{ classType: object.type as Record<string, unknown> }]
    children.push(...attrChildren(object.attrs as unknown[], 'ClassInstance'))
    return this.enter(object, { kind: 'class-instance', instanceId }, children)
  }

  /**
   * Opens a frame for a class node, or reuses one: the same `classType`
   * object gives the same node, and a class with no eager attrs has one node
   * per id however it is spelled. A class met again while its own attrs are
   * being pushed (a class constant that is an instance of the class) gets an
   * attr-less duplicate rather than a cycle error, as the sandbox's export does.
   */
  private enterClassType(object: Record<string, unknown>): number | Frame {
    // Require an array like the native binding does, so both transports
    // enforce the same marker contract (a missing `attrs` is a forged or
    // malformed marker, not an empty attribute list).
    if (!Array.isArray(object.attrs)) {
      throw new TypeError('ClassType attrs must be an array of [name, value] pairs')
    }
    const header: ClassHeader = {
      name: String(object.name),
      id: uuidString(object.id, 'ClassType id'),
      hostDefined: object.hostDefined === true,
      isDataclass: object.isDataclass === true,
    }
    const seen = this.memo.get(object)
    if (seen === IN_PROGRESS) {
      return this.leaf({ tag: 'class-type', val: { ...header, attrs: [] } })
    }
    if (seen !== undefined) {
      return seen
    }
    const children = attrChildren(object.attrs as unknown[], 'ClassType')
    if (children.length === 0) {
      const shared = this.classTypes.get(header.id)
      if (shared !== undefined) {
        this.memo.set(object, shared)
        return shared
      }
    }
    this.memo.set(object, IN_PROGRESS)
    return { pending: { kind: 'class-type', header }, key: object, children, next: 0, ids: [] }
  }

  /** Builds a container's node once every child index is known. */
  private complete(frame: Frame): number {
    const { pending, ids } = frame
    let node: ValueNode
    switch (pending.kind) {
      case 'list-value':
      case 'tuple-value':
      case 'set':
        node = { tag: pending.kind, val: Uint32Array.from(ids) }
        break
      case 'dict':
        node = { tag: 'dict', val: indexPairs(ids) }
        break
      case 'class-type':
        node = { tag: 'class-type', val: { ...pending.header, attrs: indexPairs(ids) } }
        if (ids.length === 0 && !this.classTypes.has(pending.header.id)) {
          this.classTypes.set(pending.header.id, this.nodes.length)
        }
        break
      case 'class-instance':
        node = {
          tag: 'class-instance',
          val: { classType: ids[0]!, instanceId: pending.instanceId, attrs: indexPairs(ids.slice(1)) },
        }
        break
    }
    const index = this.leaf(node)
    this.memo.set(frame.key, index)
    return index
  }
}

/** A value still to be pushed: a JavaScript value, or the plain `classType`
 *  object of an instance or `Type` marker. */
type Child = { value: unknown } | { classType: Record<string, unknown> }

/** The fields of a `classType` object other than its attrs. */
interface ClassHeader {
  name: string
  id: string
  hostDefined: boolean
  isDataclass: boolean
}

/** What a frame builds once its children are pushed. */
type Pending =
  | { kind: 'list-value' | 'tuple-value' | 'set' }
  /** Children alternate key, value. */
  | { kind: 'dict' }
  /** Children alternate attr name, value. */
  | { kind: 'class-type'; header: ClassHeader }
  /** The first child is the class node, then attr name, value pairs. */
  | { kind: 'class-instance'; instanceId: string }

/** A container mid-encoding: its children are pushed one at a time on the
 *  explicit stack, then the node is built from their indexes. */
interface Frame {
  pending: Pending
  /** The memoized object. */
  key: object
  children: Child[]
  /** Index of the next child to push. */
  next: number
  /** Indexes of the children pushed so far. */
  ids: number[]
}

function asChild(value: unknown): Child {
  return { value }
}

/** Key/value entries as pending children, key then value. */
function pairChildren(pairs: [unknown, unknown][]): Child[] {
  return pairs.flatMap(([key, value]) => [{ value: key }, { value }])
}

/** The `[name, value]` pairs attrs cross as, as pending children (name then
 *  value); a malformed pair is rejected with napi's message. */
function attrChildren(attrs: unknown[], typeName: string): Child[] {
  const children: Child[] = []
  for (const pair of attrs) {
    if (!Array.isArray(pair)) throw new TypeError(`${typeName} attrs entries must be [name, value] pairs`)
    if (typeof pair[0] !== 'string') throw new TypeError(`${typeName} attr name must be a string`)
    if (!(1 in pair)) throw new TypeError(`${typeName} attr value missing`)
    children.push({ value: pair[0] }, { value: pair[1] })
  }
  return children
}

/** Regroups the indexes of children pushed key, value, key, value, … into pairs. */
function indexPairs(ids: number[]): NodePair[] {
  const pairs: NodePair[] = []
  for (let i = 0; i + 1 < ids.length; i += 2) {
    pairs.push({ key: ids[i]!, value: ids[i + 1]! })
  }
  return pairs
}

/** Preserves aware-time metadata while rejecting an orphaned timezone name. */
function timeZoneFields(
  object: Record<string, unknown>,
  typeName: 'DateTime' | 'Time',
): { offsetSeconds?: number; timezoneName?: string } {
  const aware = object.offsetSeconds !== undefined && object.offsetSeconds !== null
  if (!aware && object.timezoneName !== undefined && object.timezoneName !== null) {
    throw new TypeError(`Monty${typeName} timezoneName requires offsetSeconds`)
  }
  return aware
    ? {
        offsetSeconds: Number(object.offsetSeconds),
        ...(typeof object.timezoneName === 'string' ? { timezoneName: object.timezoneName } : {}),
      }
    : {}
}

/** A canonical uuid string is required for identities crossing the wire. */
function uuidString(value: unknown, what: string): string {
  if (
    typeof value !== 'string' ||
    !/^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/.test(value)
  ) {
    throw new TypeError(`${what} must be a canonical uuid string`)
  }
  return value.toLowerCase()
}

/** Validates and converts a sandbox file-handle marker. */
function fileHandleNode(object: Record<string, unknown>): ValueNode {
  if (typeof object.path !== 'string') throw new TypeError('MontyFileHandle path must be a string')
  if (typeof object.mode !== 'string') throw new TypeError('MontyFileHandle mode must be a string')
  const position = object.position === undefined ? 0 : object.position
  validateFilePosition(position)
  return {
    tag: 'file-handle',
    val: { path: object.path, mode: canonicalFileMode(object.mode), position: BigInt(position) },
  }
}

/**
 * Decodes one message's arena into JavaScript values and returns a lookup
 * by node index. Every node is converted once, in arena order, so a child
 * always exists before its holder and a node referenced twice is one
 * JavaScript object; instances of one class share its `classType` object. A
 * child index that is not lower than its holder's is rejected.
 */
export function decodeArena(arena: Arena): (index: number) => unknown {
  const built: unknown[] = []
  const child = (index: number, holder: number): unknown => {
    if (!(index < holder) || !Number.isInteger(index) || index < 0) {
      throw new Error(`component value node index ${index} is not lower than its holder ${holder}`)
    }
    return built[index]
  }
  for (const [holder, node] of arena.nodes.entries()) {
    built.push(decodeNode(node, holder, child))
  }
  return (index) => {
    if (index >= built.length) {
      throw new Error(`component value node index ${index} is out of bounds`)
    }
    return built[index]
  }
}

/** Converts one node, resolving its children (already built) through `child`. */
function decodeNode(node: ValueNode, holder: number, child: (index: number, holder: number) => unknown): unknown {
  const items = (indexes: Uint32Array): unknown[] => [...indexes].map((index) => child(index, holder))
  const pair = ({ key, value }: NodePair): [unknown, unknown] => [child(key, holder), child(value, holder)]
  switch (node.tag) {
    case 'ellipsis':
      return { [TYPE_MARKER]: 'Ellipsis' }
    case 'not-implemented':
      return { [TYPE_MARKER]: 'NotImplemented' }
    case 'none':
      return null
    case 'boolean':
    case 'float':
    case 'text':
      return node.val
    case 'integer':
      return node.val >= -SAFE && node.val <= SAFE ? Number(node.val) : node.val
    case 'bigint':
      return BigInt(node.val)
    case 'bytes':
      return typeof Buffer === 'undefined' ? node.val : Buffer.from(node.val)
    case 'list-value':
      return items(node.val)
    case 'tuple-value':
      return asTuple(items(node.val))
    case 'named-tuple':
      return asTuple(items(node.val.items))
    case 'dict':
      return new Map(node.val.map(pair))
    case 'set':
    case 'frozen-set':
      return new Set(items(node.val))
    case 'date':
      return { [TYPE_MARKER]: 'Date', ...node.val }
    case 'datetime':
      return { [TYPE_MARKER]: 'DateTime', ...node.val }
    case 'time':
      return { [TYPE_MARKER]: 'Time', ...node.val }
    case 'timedelta':
      return { [TYPE_MARKER]: 'TimeDelta', ...node.val }
    case 'timezone':
      return { [TYPE_MARKER]: 'TimeZone', ...node.val }
    case 'exception':
      return { [TYPE_MARKER]: 'Exception', excType: node.val.excType, message: node.val.message ?? '' }
    case 'type-name':
      return { [TYPE_MARKER]: 'Type', value: node.val }
    case 'class-type':
      return {
        [TYPE_MARKER]: 'Type',
        classType: {
          name: node.val.name,
          id: node.val.id,
          hostDefined: node.val.hostDefined,
          isDataclass: node.val.isDataclass,
          attrs: attrPairs(node.val.attrs, pair),
        },
      }
    case 'builtin-function':
      return { [TYPE_MARKER]: 'BuiltinFunction', value: node.val }
    case 'path':
    case 'repr':
    case 'cycle':
      return node.val
    case 'file-handle':
      if (node.val.position > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new TypeError("MontyFileHandle position exceeds JavaScript's maximum safe integer")
      }
      return new MontyFileHandle(node.val.path, node.val.mode, { position: Number(node.val.position) })
    case 'class-instance': {
      // the marker built for the instance's class node, whose `classType`
      // object every instance of the class shares
      const classMarker = child(node.val.classType, holder) as { classType?: unknown }
      if (typeof classMarker !== 'object' || classMarker === null || classMarker.classType === undefined) {
        throw new Error("class-instance node's class-type index is not a class-type node")
      }
      return {
        [TYPE_MARKER]: 'ClassInstance',
        type: classMarker.classType,
        instanceId: node.val.instanceId,
        attrs: attrPairs(node.val.attrs, pair),
      }
    }
    case 'function':
      return node.val.name
  }
}

/** Attrs as ordered `[name, value]` pairs in the shape the napi path
 *  produces; non-string keys are not representable host-side and are skipped. */
function attrPairs(attrs: NodePair[], pair: (pair: NodePair) => [unknown, unknown]): [string, unknown][] {
  const out: [string, unknown][] = []
  for (const entry of attrs) {
    const [key, value] = pair(entry)
    if (typeof key === 'string') out.push([key, value])
  }
  return out
}

/** Stamps the non-enumerable tuple marker used by both JS transports. */
function asTuple(items: unknown[]): unknown[] {
  Object.defineProperty(items, TUPLE_MARKER, { value: true, enumerable: false })
  return items
}

/** Whether an input array represents a Python tuple. */
function isTuple(array: unknown[]): boolean {
  return (array as { [TUPLE_MARKER]?: unknown })[TUPLE_MARKER] === true
}

/** Creates the established unsupported-value conversion error. */
function unsupported(what: string): Error {
  return new Error(`monty wasm transport does not support ${what}`)
}

/** Produces napi-compatible JavaScript type names for conversion errors. */
function jsType(value: unknown): string {
  if (value === undefined) {
    return 'Undefined'
  } else if (value === null) {
    return 'Null'
  } else if (Array.isArray(value)) {
    return 'Array'
  } else if (typeof value === 'bigint') {
    return 'BigInt'
  } else {
    return typeof value === 'object' ? 'Object' : typeof value
  }
}
