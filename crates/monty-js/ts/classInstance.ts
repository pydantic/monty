// Policy wrapper exposing a host object to the Monty sandbox, plus the
// per-session instance store and the outbound/inbound value walks
// (`prepare` / `restore`) that carry class instances across the boundary.
//
// Mirrors pydantic_monty's ClassInstance: the wrapper decides which
// attributes cross eagerly, which may be fetched lazily, and which methods
// sandbox code may call. The sandbox routes method calls and lazy attribute
// lookups back to the wrapped object by instance id, and when sandbox code
// returns the instance, the host receives the original object back
// (identity preserved). Instances with no host original — defined inside the
// sandbox, or from a session restored into a fresh process — surface as
// read-only [`MontyClassInstance`] proxies.

import { notCallableMessage } from './errors.js'

/**
 * Which names a wrapper exposes: an explicit collection, or `'all'` for every
 * non-underscore name. Absent (undefined) means none.
 */
export type AttrPolicy = readonly string[] | ReadonlySet<string> | 'all'

/** Options for [`ClassInstance`] — all policies default to "expose nothing". */
export interface ClassInstanceOptions {
  /** Attributes sent into the sandbox with the instance: `'all'` sends the
   *  instance's own enumerable string-keyed props (skipping `_`-prefixed
   *  names), an explicit list reads exactly those props. */
  eagerAttrs?: AttrPolicy
  /** Attributes the sandbox may fetch on demand (prototype getters/fields
   *  included); `'all'` never exposes `_`-prefixed names. */
  lazyAttrs?: AttrPolicy
  /** Methods the sandbox may call; `'all'` never exposes `_`-prefixed names. */
  allowedMethods?: AttrPolicy
  /** Class name shown to the sandbox; defaults to the constructor name. */
  name?: string
  /** Whether sandbox `setattr` raises `FrozenInstanceError` (default false). */
  frozen?: boolean
  /**
   * Transforms each value crossing to the sandbox — applied exactly once per
   * value: to each eager attr, each lazy lookup result, and each method
   * return value (after the promise settles, for async methods). Replaces the
   * default conversion, which auto-wraps non-plain objects via
   * [`ClassInstance.childWrapper`].
   */
  convertValue?: (name: string, value: unknown) => unknown
}

/**
 * Policy wrapper exposing a host class instance to the Monty sandbox. Pass it
 * as an input or return it from an external function:
 *
 * ```ts
 * await session.feedRun('assert user.greeting() == "hi Sam"', {
 *   inputs: { user: new ClassInstance(user, { eagerAttrs: 'all', allowedMethods: ['greeting'] }) },
 * })
 * ```
 */
export class ClassInstance {
  constructor(
    /** The wrapped host object; handed back as-is when the sandbox returns the instance. */
    readonly instance: object,
    readonly options: ClassInstanceOptions = {},
  ) {
    if (typeof instance !== 'object' || instance === null) {
      throw new TypeError('ClassInstance expects an object instance')
    }
  }

  /** Class name shown to the sandbox: `options.name`, else the constructor name. */
  getName(): string {
    if (this.options.name !== undefined) {
      return this.options.name
    }
    const ctor = (this.instance as { constructor?: { name?: unknown } }).constructor
    return typeof ctor?.name === 'string' && ctor.name !== '' ? ctor.name : 'object'
  }

  /** The `[name, value]` attr pairs sent into the sandbox with the instance,
   *  each value already passed through `convertValue`. */
  getEagerAttrs(): Array<[string, unknown]> {
    const policy = this.options.eagerAttrs
    if (policy === undefined) {
      return []
    }
    const source = this.instance as Record<string, unknown>
    const names = policy === 'all' ? Object.keys(source).filter((name) => !name.startsWith('_')) : [...policy]
    return names.map((name) => [name, this.convertValue(name, source[name])])
  }

  /**
   * Resolves a lazy attribute lookup from the sandbox. Throws the internal
   * [`AttrNotExposed`] sentinel when `name` is outside `lazyAttrs` or the
   * property is absent; the sandbox then raises `AttributeError`.
   */
  lookupLazyAttr(name: string): unknown {
    if (!policyAllows(this.options.lazyAttrs, name) || !(name in this.instance)) {
      throw new AttrNotExposed(this.getName(), name)
    }
    return this.convertValue(name, (this.instance as Record<string, unknown>)[name])
  }

  /**
   * Calls a method on the wrapped instance for the sandbox. Throws
   * [`AttrNotExposed`] when `name` is outside `allowedMethods` or absent.
   *
   * JS functions have no keyword arguments, so a non-empty `kwargs` is
   * appended as a final options-bag argument — the way JS hosts typically
   * take named options. The return value passes through `convertValue`
   * (after settling, for a promise-returning method).
   */
  callMethod(name: string, args: unknown[], kwargs: Record<string, unknown>): unknown {
    if (!policyAllows(this.options.allowedMethods, name) || !(name in this.instance)) {
      throw new AttrNotExposed(this.getName(), name)
    }
    const method = (this.instance as Record<string, unknown>)[name]
    if (typeof method !== 'function') {
      throw new TypeError(notCallableMessage(method))
    }
    const callArgs = Object.keys(kwargs).length > 0 ? [...args, kwargs] : args
    const result = method.apply(this.instance, callArgs)
    return isThenable(result)
      ? Promise.resolve(result).then((value) => this.convertValue(name, value))
      : this.convertValue(name, result)
  }

  /**
   * Transforms one value crossing to the sandbox (see
   * [`ClassInstanceOptions.convertValue`]). The default auto-wraps non-plain
   * objects — anything [`prepare`] would reject — in [`childWrapper`], so
   * methods returning class instances work without ceremony.
   */
  convertValue(name: string, value: unknown): unknown {
    if (this.options.convertValue !== undefined) {
      return this.options.convertValue(name, value)
    }
    if (typeof value === 'object' && value !== null && !(value instanceof ClassInstance) && !isNativeObject(value)) {
      return this.childWrapper(value)
    }
    return value
  }

  /** Wraps a derived value (nested attr / method return) with this wrapper's
   *  exposure policies; `name` and `frozen` revert to their defaults. */
  childWrapper(value: object): ClassInstance {
    const { eagerAttrs, lazyAttrs, allowedMethods, convertValue } = this.options
    return new ClassInstance(value, { eagerAttrs, lazyAttrs, allowedMethods, convertValue })
  }
}

/**
 * Read-only stand-in for a class instance the host has no original object
 * for: one defined inside the sandbox, or a host instance returned after the
 * session was restored into a fresh process.
 */
export class MontyClassInstance {
  /** Class name of the instance (e.g. `'Point'`). */
  readonly name: string
  /** Whether the instance was a dataclass on the side that produced it. */
  readonly isDataclass: boolean
  /** The instance's attributes. Null-prototype record: attr names are
   *  sandbox-controlled, so they must never become prototype properties. */
  readonly attributes: Record<string, unknown>

  /** @internal — built by `restore` from the wire marker. */
  constructor(name: string, isDataclass: boolean, attrs: Array<[string, unknown]>) {
    this.name = name
    this.isDataclass = isDataclass
    const attributes: Record<string, unknown> = Object.create(null)
    for (const [key, value] of attrs) {
      if (typeof key === 'string') {
        attributes[key] = value
      }
    }
    this.attributes = attributes
  }
}

/**
 * Per-session map from instance id to the [`ClassInstance`] wrapper that sent
 * it. Populated by [`prepare`]; consulted to answer method calls and lazy
 * attribute lookups, and to hand the original object back when the sandbox
 * returns the instance. Holding the wrapper keeps the instance (and its id)
 * alive for the session, mirroring pydantic_monty's `InstanceStore`.
 */
export class InstanceStore {
  /** id → wrapper; last wrapper wins when the same object is re-sent. */
  readonly map = new Map<bigint, ClassInstance>()

  /** Registers a wrapper and returns the instance's session-stable id. */
  register(wrapper: ClassInstance): bigint {
    const id = idFor(instanceIds, wrapper.instance)
    this.map.set(id, wrapper)
    return id
  }

  /** Looks up the wrapper registered for `id`. */
  get(id: bigint): ClassInstance | undefined {
    return this.map.get(id)
  }
}

/**
 * Internal sentinel thrown by [`ClassInstance.lookupLazyAttr`] /
 * [`ClassInstance.callMethod`] when a name is outside the wrapper's policy or
 * absent; the session layer turns it into a sandbox `AttributeError`. Not
 * exported from the package index.
 */
export class AttrNotExposed extends Error {
  constructor(typeName: string, attrName: string) {
    super(attributeErrorMessage(typeName, attrName))
    this.name = 'AttrNotExposed'
  }
}

/** CPython's `AttributeError` message for a missing/denied attribute. */
export function attributeErrorMessage(typeName: string, attrName: string): string {
  return `'${typeName}' object has no attribute '${attrName}'`
}

/**
 * Outbound walk over a host value heading into the sandbox: replaces
 * [`ClassInstance`] wrappers with their wire marker (registering them in
 * `store`, eager attrs prepared recursively), recurses into arrays / Maps /
 * Sets / plain objects, and rejects any other non-plain object with a
 * `TypeError` telling the caller to wrap it. Untouched values return the
 * identical reference — nothing is copied when there is nothing to do.
 */
export function prepare(value: unknown, store: InstanceStore): unknown {
  if (typeof value !== 'object' || value === null) {
    return value
  }
  if (value instanceof ClassInstance) {
    return wrapperToMarker(value, store)
  }
  if (Array.isArray(value)) {
    return walkArray(value, store, prepare)
  }
  if (value instanceof Map) {
    return walkMap(value, store, prepare)
  }
  if (value instanceof Set) {
    return walkSet(value, store, prepare)
  }
  if (value instanceof Uint8Array || hasTypeMarker(value)) {
    return value
  }
  if (isPlainObject(value)) {
    return walkPlainObject(value as Record<string, unknown>, store, prepare)
  }
  throw new TypeError(
    `Cannot convert ${constructorName(value)} instance to a Monty value — wrap it in ClassInstance(...)`,
  )
}

/**
 * Inbound walk over a sandbox value reaching the host: maps `ClassInstance`
 * markers to the original wrapped object when the id is in `store` (identity
 * preserved), else to a [`MontyClassInstance`] proxy with recursively
 * restored attrs; recurses into containers with the same no-copy-when-
 * unchanged behaviour as [`prepare`].
 */
export function restore(value: unknown, store: InstanceStore): unknown {
  if (typeof value !== 'object' || value === null) {
    return value
  }
  if (Array.isArray(value)) {
    return walkArray(value, store, restore)
  }
  if (value instanceof Map) {
    return walkMap(value, store, restore)
  }
  if (value instanceof Set) {
    return walkSet(value, store, restore)
  }
  if (value instanceof Uint8Array) {
    return value
  }
  const marker = readTypeMarker(value)
  if (marker === 'ClassInstance') {
    return markerToInstance(value as Record<string, unknown>, store)
  }
  if (marker === undefined && isPlainObject(value)) {
    return walkPlainObject(value as Record<string, unknown>, store, restore)
  }
  return value
}

/** Registers `wrapper` and builds its wire marker, preparing eager attrs
 *  recursively so nested wrappers register themselves too. */
function wrapperToMarker(wrapper: ClassInstance, store: InstanceStore): Record<string, unknown> {
  const instanceId = store.register(wrapper)
  const attrs = wrapper.getEagerAttrs().map(([name, value]): [string, unknown] => [name, prepare(value, store)])
  return {
    __monty_type__: 'ClassInstance',
    name: wrapper.getName(),
    instanceId,
    typeId: typeIdFor(wrapper.instance),
    attrs,
    frozen: wrapper.options.frozen ?? false,
    // JS has no dataclasses; host-wrapped objects always cross as plain classes
    isDataclass: false,
  }
}

/** Maps an inbound `ClassInstance` marker to the original instance or a proxy. */
function markerToInstance(marker: Record<string, unknown>, store: InstanceStore): unknown {
  const instanceId = typeof marker.instanceId === 'bigint' ? marker.instanceId : 0n
  if (instanceId !== 0n) {
    const wrapper = store.get(instanceId)
    if (wrapper !== undefined) {
      return wrapper.instance
    }
  }
  const attrs: Array<[string, unknown]> = []
  if (Array.isArray(marker.attrs)) {
    for (const pair of marker.attrs as unknown[]) {
      if (Array.isArray(pair) && typeof pair[0] === 'string') {
        attrs.push([pair[0], restore(pair[1], store)])
      }
    }
  }
  return new MontyClassInstance(
    typeof marker.name === 'string' ? marker.name : 'object',
    marker.isDataclass === true,
    attrs,
  )
}

// === shared container walks (used by both prepare and restore) ===

type Walk = (value: unknown, store: InstanceStore) => unknown

/** Walks array items, preserving the `__tuple__` marker on a rebuilt array. */
function walkArray(array: unknown[], store: InstanceStore, walk: Walk): unknown[] {
  let changed = false
  const items = array.map((item) => {
    const out = walk(item, store)
    changed ||= out !== item
    return out
  })
  if (!changed) {
    return array
  }
  if ((array as { __tuple__?: unknown }).__tuple__ === true) {
    Object.defineProperty(items, '__tuple__', { value: true, enumerable: false })
  }
  return items
}

function walkMap(map: Map<unknown, unknown>, store: InstanceStore, walk: Walk): Map<unknown, unknown> {
  let changed = false
  const out = new Map<unknown, unknown>()
  for (const [key, value] of map) {
    const outKey = walk(key, store)
    const outValue = walk(value, store)
    changed ||= outKey !== key || outValue !== value
    out.set(outKey, outValue)
  }
  return changed ? out : map
}

function walkSet(set: Set<unknown>, store: InstanceStore, walk: Walk): Set<unknown> {
  let changed = false
  const out = new Set<unknown>()
  for (const item of set) {
    const outItem = walk(item, store)
    changed ||= outItem !== item
    out.add(outItem)
  }
  return changed ? out : set
}

/** Walks a plain object's own enumerable entries; a rebuilt object has a null
 *  prototype so no key can land on `Object.prototype`. */
function walkPlainObject(obj: Record<string, unknown>, store: InstanceStore, walk: Walk): Record<string, unknown> {
  let changed = false
  const out: Record<string, unknown> = Object.create(null)
  for (const [key, value] of Object.entries(obj)) {
    const outValue = walk(value, store)
    changed ||= outValue !== value
    out[key] = outValue
  }
  return changed ? out : obj
}

// === identity / classification helpers ===

/** Module-level id mint shared by instance and type ids; 0 is reserved for
 *  sandbox-defined instances, so ids start at 1. The WeakMaps make ids stable
 *  per object without pinning the objects themselves. */
let nextId = 1n
const instanceIds = new WeakMap<object, bigint>()
const typeIds = new WeakMap<object, bigint>()

function idFor(ids: WeakMap<object, bigint>, key: object): bigint {
  let id = ids.get(key)
  if (id === undefined) {
    id = nextId++
    ids.set(key, id)
  }
  return id
}

/** Stable id for the instance's class (its constructor), 0n when it has none. */
function typeIdFor(instance: object): bigint {
  const ctor = (instance as { constructor?: unknown }).constructor
  return typeof ctor === 'function' ? idFor(typeIds, ctor as object) : 0n
}

/** Whether `policy` exposes `name`; `'all'` never exposes underscore names. */
function policyAllows(policy: AttrPolicy | undefined, name: string): boolean {
  if (policy === undefined) {
    return false
  }
  if (policy === 'all') {
    return !name.startsWith('_')
  }
  return policy instanceof Set ? policy.has(name) : (policy as readonly string[]).includes(name)
}

/** True when the object's prototype is `Object.prototype` or `null`. */
function isPlainObject(value: object): boolean {
  const proto: unknown = Object.getPrototypeOf(value)
  return proto === Object.prototype || proto === null
}

/** True when the js↔monty conversion accepts the object without a wrapper. */
function isNativeObject(value: object): boolean {
  return (
    isPlainObject(value) ||
    Array.isArray(value) ||
    value instanceof Map ||
    value instanceof Set ||
    value instanceof Uint8Array ||
    hasTypeMarker(value)
  )
}

function hasTypeMarker(value: object): boolean {
  return typeof readTypeMarker(value) === 'string'
}

/** Reads `__monty_type__` without letting a throwing getter escape (mirrors
 *  `readMarker` in errors.ts — an exotic host value must degrade, not throw). */
function readTypeMarker(value: object): string | undefined {
  try {
    const marker = (value as { __monty_type__?: unknown }).__monty_type__
    return typeof marker === 'string' ? marker : undefined
  } catch {
    return undefined
  }
}

function constructorName(value: object): string {
  const ctor = (value as { constructor?: { name?: unknown } }).constructor
  return typeof ctor?.name === 'string' && ctor.name !== '' ? ctor.name : 'object'
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return typeof value === 'object' && value !== null && typeof (value as { then?: unknown }).then === 'function'
}
