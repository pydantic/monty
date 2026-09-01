// Policy wrapper exposing a host object to the Monty sandbox, plus the
// per-session instance store and the outbound/inbound value walks
// (`prepare` / `restore`) that carry class instances across the boundary.
//
// Mirrors pydantic_monty's ClassInstance: the wrapper decides which
// attributes cross eagerly, which may be fetched lazily, and which methods
// sandbox code may call. The sandbox routes method calls and lazy attribute
// lookups back to the wrapped object by a session-local uuid (never a memory
// address), and when sandbox code returns the instance, the host receives
// the original object back (identity preserved). Instances with no host
// original — defined inside the sandbox, or from a session restored into a
// fresh process — surface as read-only [`MontyClassProxy`] stand-ins.
//
// [`ClassType`] is the class-level sibling: wrap a class to pass it into the
// sandbox, optionally letting sandbox code instantiate it (`init: true`).

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
   * return value (after the promise settles, for async methods). The default
   * passes values through unchanged, so a derived non-plain object fails with
   * the usual "wrap it in ClassInstance" TypeError — use this hook to wrap
   * such values with policies chosen per value.
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
   * [`ClassInstanceOptions.convertValue`]). The default passes values through
   * unchanged — deliberately no automatic wrapping: each object's exposure
   * must be an explicit host decision, since a wrapper inheriting this
   * wrapper's policies could silently widen access to an instance the host
   * had locked down elsewhere.
   */
  convertValue(name: string, value: unknown): unknown {
    if (this.options.convertValue !== undefined) {
      return this.options.convertValue(name, value)
    }
    return value
  }
}

/** Options for [`ClassType`]: the `init` gate plus the instance policies
 *  applied to every instance the class constructs for the sandbox. */
export interface ClassTypeOptions extends ClassInstanceOptions {
  /** Whether sandbox code may instantiate the class (default false). Purely
   *  a host-side policy: it never crosses the wire, and `construct` checks
   *  it on every request. */
  init?: boolean
}

/**
 * Policy wrapper exposing a host *class* to the Monty sandbox. With
 * `init: true`, sandbox code may call the class to construct instances; the
 * construction runs host-side and the result crosses back wrapped in a
 * [`ClassInstance`] carrying this wrapper's instance policies:
 *
 * ```ts
 * await session.feedRun('p = Point(1, 2)\nassert p.x == 1', {
 *   inputs: { Point: new ClassType(Point, { init: true, eagerAttrs: 'all' }) },
 * })
 * ```
 */
export class ClassType {
  constructor(
    /** The wrapped host class (a constructor function). */
    readonly classType: new (...args: never[]) => object,
    readonly options: ClassTypeOptions = {},
  ) {
    if (typeof classType !== 'function') {
      throw new TypeError('ClassType expects a class (constructor function)')
    }
  }

  /** Class name shown to the sandbox: `options.name`, else the class name. */
  getName(): string {
    if (this.options.name !== undefined) {
      return this.options.name
    }
    return typeof this.classType.name === 'string' && this.classType.name !== '' ? this.classType.name : 'object'
  }

  /**
   * Constructs an instance for the sandbox, checking the `init` policy —
   * a purely host-side gate that never crosses the wire. JS constructors
   * have no keyword arguments, so a non-empty `kwargs` is appended as a
   * final options bag, matching [`ClassInstance.callMethod`].
   */
  construct(args: unknown[], kwargs: Record<string, unknown>): ClassInstance {
    if (this.options.init !== true) {
      throw new TypeError(`cannot instantiate host class '${this.getName()}'`)
    }
    const callArgs = Object.keys(kwargs).length > 0 ? [...args, kwargs] : args
    return this.instanceWrapper(new this.classType(...(callArgs as never[])))
  }

  /** Wraps a constructed instance with this wrapper's instance policies.
   *  Override to customize how constructed instances are exposed. */
  instanceWrapper(instance: object): ClassInstance {
    const { eagerAttrs, lazyAttrs, allowedMethods, frozen, convertValue } = this.options
    return new ClassInstance(instance, { eagerAttrs, lazyAttrs, allowedMethods, frozen, convertValue })
  }
}

/**
 * Read-only stand-in for a class instance the host has no original object
 * for: one defined inside the sandbox, or a host instance returned after the
 * session was restored into a fresh process.
 */
export class MontyClassProxy {
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
  /** uuid → wrapper; last wrapper wins when the same object is re-sent. */
  readonly map = new Map<string, ClassInstance>()
  /** class uuid → the class plus the `ClassType` wrapper gating
   *  instantiation (undefined until one crosses). Pins the class so its
   *  uuid dedup stays sound. */
  readonly classes = new Map<string, { classObject: object; wrapper?: ClassType }>()
  /** Per-session uuid mints: the WeakMaps make ids stable per object within
   *  this store without pinning the objects themselves (registrations do the
   *  pinning where soundness needs it). Session-local on purpose — a shared
   *  mint would let a compromised worker recognise the same host object
   *  across checkouts. */
  private readonly instanceIds = new WeakMap<object, string>()
  private readonly classIds = new WeakMap<object, string>()

  /** Registers a wrapper and returns the instance's session-stable uuid. */
  register(wrapper: ClassInstance): string {
    const id = uuidFor(this.instanceIds, wrapper.instance)
    this.map.set(id, wrapper)
    return id
  }

  /** The class's session uuid, minting and pinning it on first sight. */
  typeUuid(classObject: object): string {
    const id = uuidFor(this.classIds, classObject)
    if (!this.classes.has(id)) {
      this.classes.set(id, { classObject })
    }
    return id
  }

  /** Registers a `ClassType` wrapper under its class uuid. */
  registerClass(wrapper: ClassType): string {
    const id = this.typeUuid(wrapper.classType)
    this.classes.set(id, { classObject: wrapper.classType, wrapper })
    return id
  }

  /** Looks up the wrapper registered for `id`. */
  get(id: string): ClassInstance | undefined {
    return this.map.get(id)
  }

  /**
   * Constructs an instance of the class registered for `typeId` through its
   * `ClassType` wrapper (which re-checks its own `init` policy). Throws when
   * the class never crossed with a wrapper — e.g. after a session restore.
   */
  instantiate(typeId: string, name: string, args: unknown[], kwargs: Record<string, unknown>): ClassInstance {
    const wrapper = this.classes.get(typeId)?.wrapper
    if (wrapper === undefined) {
      throw new Error(
        `no host class registered for instantiation of '${name}' (id ${typeId}) — ` +
          'pass the class as a ClassType(..., { init: true })',
      )
    }
    return wrapper.construct(args, kwargs)
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
  return prepareInner(value, store, 0)
}

/** Recursion guard for the outbound walk itself, so a too-deep value fails
 *  with a catchable error instead of a `RangeError` mid-recursion. Not the
 *  authoritative wire budget: the native layer re-checks every value with
 *  exact per-shape accounting (`exceeds_max_value_depth`) before encoding. */
const MAX_INPUT_DEPTH = 48
/** Backstop for inbound walks; wire values are already bounded well below. */
const MAX_OUTPUT_DEPTH = 200

function prepareInner(value: unknown, store: InstanceStore, depth: number): unknown {
  if (depth > MAX_INPUT_DEPTH) {
    throw new TypeError('Max input depth exceeded')
  }
  if (typeof value !== 'object' || value === null) {
    return value
  }
  if (value instanceof ClassInstance) {
    return wrapperToMarker(value, store, depth)
  }
  if (value instanceof ClassType) {
    return classTypeToMarker(value, store)
  }
  if (Array.isArray(value)) {
    return walkArray(value, store, depth, prepareInner)
  }
  if (value instanceof Map) {
    return walkMap(value, store, depth, prepareInner)
  }
  if (value instanceof Set) {
    return walkSet(value, store, depth, prepareInner)
  }
  if (value instanceof Uint8Array) {
    return value
  }
  const marker = readTypeMarker(value)
  if (marker === 'ClassInstance') {
    // Identity-bearing markers are produced internally by this walk, never
    // held by host code (`restore` maps them to the original object or a
    // MontyClassProxy). One arriving here is forged — e.g. embedded
    // in attacker-controlled JSON to impersonate a registered instance.
    throw new TypeError('raw ClassInstance markers are not accepted — wrap the object in ClassInstance(...)')
  }
  if (marker !== undefined) {
    return value
  }
  if (isPlainObject(value)) {
    return walkPlainObject(value as Record<string, unknown>, store, depth, prepareInner)
  }
  throw new TypeError(
    `Cannot convert ${constructorName(value)} instance to a Monty value — wrap it in ClassInstance(...)`,
  )
}

/**
 * Inbound walk over a sandbox value reaching the host: maps `ClassInstance`
 * markers to the original wrapped object when the id is in `store` (identity
 * preserved), else to a [`MontyClassProxy`] proxy with recursively
 * restored attrs; recurses into containers with the same no-copy-when-
 * unchanged behaviour as [`prepare`].
 */
export function restore(value: unknown, store: InstanceStore): unknown {
  return restoreInner(value, store, 0)
}

function restoreInner(value: unknown, store: InstanceStore, depth: number): unknown {
  if (depth > MAX_OUTPUT_DEPTH) {
    throw new TypeError('Max output depth exceeded')
  }
  if (typeof value !== 'object' || value === null) {
    return value
  }
  if (Array.isArray(value)) {
    return walkArray(value, store, depth, restoreInner)
  }
  if (value instanceof Map) {
    return walkMap(value, store, depth, restoreInner)
  }
  if (value instanceof Set) {
    return walkSet(value, store, depth, restoreInner)
  }
  if (value instanceof Uint8Array) {
    return value
  }
  const marker = readTypeMarker(value)
  if (marker === 'ClassInstance') {
    return markerToInstance(value as Record<string, unknown>, store, depth)
  }
  if (marker === undefined && isPlainObject(value)) {
    return walkPlainObject(value as Record<string, unknown>, store, depth, restoreInner)
  }
  return value
}

/** Registers `wrapper` and builds its wire marker, preparing eager attrs
 *  recursively so nested wrappers register themselves too. */
function wrapperToMarker(wrapper: ClassInstance, store: InstanceStore, depth: number): Record<string, unknown> {
  const instanceId = store.register(wrapper)
  const attrs = wrapper
    .getEagerAttrs()
    .map(([name, value]): [string, unknown] => [name, prepareInner(value, store, depth + 1)])
  return {
    __monty_type__: 'ClassInstance',
    type: classTypeObject(wrapper.getName(), instanceClass(wrapper.instance), wrapper.options, store),
    instanceId,
    attrs,
  }
}

/** Registers a `ClassType` wrapper and builds its `Type` wire marker. */
function classTypeToMarker(wrapper: ClassType, store: InstanceStore): Record<string, unknown> {
  store.registerClass(wrapper)
  return {
    __monty_type__: 'Type',
    classType: classTypeObject(wrapper.getName(), wrapper.classType, wrapper.options, store),
  }
}

/**
 * The plain `classType` object shared by ClassInstance and Type markers:
 * name, session uuid, the wrapper policy flags, and `parents` from the
 * constructor prototype chain (each ancestor a `Type` marker with its own
 * uuid and default flags).
 */
function classTypeObject(
  name: string,
  classObject: object | undefined,
  options: ClassInstanceOptions,
  store: InstanceStore,
): Record<string, unknown> {
  // An object with no constructor still needs a class identity: key it on
  // `Object.prototype` so repeats stay stable within the session.
  const id = store.typeUuid(classObject ?? Object.prototype)
  return {
    name,
    id,
    hostDefined: true,
    parents: classObject === undefined ? [] : parentMarkers(classObject, store),
    // JS has no dataclasses; host-wrapped objects always cross as plain classes
    isDataclass: false,
    frozen: options.frozen ?? false,
  }
}

/** `Type` markers for the constructor prototype chain (single inheritance). */
function parentMarkers(classObject: object, store: InstanceStore): Array<Record<string, unknown>> {
  const parents: Array<Record<string, unknown>> = []
  let parent: unknown = Object.getPrototypeOf(classObject)
  while (typeof parent === 'function' && parent !== Function.prototype) {
    const parentName = typeof parent.name === 'string' && parent.name !== '' ? parent.name : 'object'
    parents.push({
      name: parentName,
      id: store.typeUuid(parent as object),
      hostDefined: true,
      parents: [],
      isDataclass: false,
      frozen: false,
    })
    parent = Object.getPrototypeOf(parent)
  }
  // The chain is single inheritance, so each ancestor is a parent of the
  // previous one; the wire carries only direct bases, so nest them.
  for (let i = parents.length - 1; i > 0; i--) {
    parents[i - 1].parents = [{ __monty_type__: 'Type', classType: parents[i] }]
  }
  return parents.length > 0 ? [{ __monty_type__: 'Type', classType: parents[0] }] : []
}

/** The instance's class (its constructor), if it has one. */
function instanceClass(instance: object): object | undefined {
  const ctor = (instance as { constructor?: unknown }).constructor
  return typeof ctor === 'function' ? (ctor as object) : undefined
}

/** Maps an inbound `ClassInstance` marker to the original instance or a proxy. */
function markerToInstance(marker: Record<string, unknown>, store: InstanceStore, depth: number): unknown {
  if (typeof marker.instanceId === 'string') {
    const wrapper = store.get(marker.instanceId)
    if (wrapper !== undefined) {
      return wrapper.instance
    }
  }
  const attrs: Array<[string, unknown]> = []
  if (Array.isArray(marker.attrs)) {
    for (const pair of marker.attrs as unknown[]) {
      if (Array.isArray(pair) && typeof pair[0] === 'string') {
        attrs.push([pair[0], restoreInner(pair[1], store, depth + 1)])
      }
    }
  }
  const classType = (marker.type ?? {}) as Record<string, unknown>
  return new MontyClassProxy(
    typeof classType.name === 'string' ? classType.name : 'object',
    classType.isDataclass === true,
    attrs,
  )
}

// === shared container walks (used by both prepare and restore) ===

type Walk = (value: unknown, store: InstanceStore, depth: number) => unknown

/** Walks array items, preserving the `__tuple__` marker on a rebuilt array. */
function walkArray(array: unknown[], store: InstanceStore, depth: number, walk: Walk): unknown[] {
  let changed = false
  const items = array.map((item) => {
    const out = walk(item, store, depth + 1)
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

function walkMap(map: Map<unknown, unknown>, store: InstanceStore, depth: number, walk: Walk): Map<unknown, unknown> {
  let changed = false
  const out = new Map<unknown, unknown>()
  for (const [key, value] of map) {
    const outKey = walk(key, store, depth + 1)
    const outValue = walk(value, store, depth + 1)
    changed ||= outKey !== key || outValue !== value
    out.set(outKey, outValue)
  }
  return changed ? out : map
}

function walkSet(set: Set<unknown>, store: InstanceStore, depth: number, walk: Walk): Set<unknown> {
  let changed = false
  const out = new Set<unknown>()
  for (const item of set) {
    const outItem = walk(item, store, depth + 1)
    changed ||= outItem !== item
    out.add(outItem)
  }
  return changed ? out : set
}

/** Walks a plain object's own enumerable entries; a rebuilt object has a null
 *  prototype so no key can land on `Object.prototype`. */
function walkPlainObject(
  obj: Record<string, unknown>,
  store: InstanceStore,
  depth: number,
  walk: Walk,
): Record<string, unknown> {
  let changed = false
  const out: Record<string, unknown> = Object.create(null)
  for (const [key, value] of Object.entries(obj)) {
    const outValue = walk(value, store, depth + 1)
    changed ||= outValue !== value
    out[key] = outValue
  }
  return changed ? out : obj
}

// === identity / classification helpers ===

/** Returns the uuid minted for `key` in `ids`, minting on first sight. */
function uuidFor(ids: WeakMap<object, string>, key: object): string {
  let id = ids.get(key)
  if (id === undefined) {
    id = mintUuid()
    ids.set(key, id)
  }
  return id
}

/** Mints a canonical lowercase uuid4 string. `getRandomValues` rather than
 *  `randomUUID` so insecure browser contexts work too. */
function mintUuid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 0x0f) | 0x40 // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80 // RFC 4122 variant
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

/** Whether `policy` exposes `name`; `'all'` never exposes underscore names. */
function policyAllows(policy: AttrPolicy | undefined, name: string): boolean {
  if (policy === undefined) {
    return false
  }
  if (policy === 'all') {
    return !name.startsWith('_')
  }
  // Duck-type on `.has` rather than `instanceof Set` so set-likes from
  // another realm (iframe / VM context) work too.
  return typeof (policy as { has?: unknown }).has === 'function'
    ? (policy as ReadonlySet<string>).has(name)
    : (policy as readonly string[]).includes(name)
}

/** True when the object's prototype is `Object.prototype` or `null`. */
function isPlainObject(value: object): boolean {
  const proto: unknown = Object.getPrototypeOf(value)
  return proto === Object.prototype || proto === null
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
