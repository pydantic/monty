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
// fresh session — cross to the host as read-only [`MontyClassProxy`] stand-ins.
//
// [`ClassType`] is the class-level sibling: wrap a class to pass it into the
// sandbox, optionally letting sandbox code instantiate it (`init: true`).

import { notCallableMessage } from './errors.js'

/**
 * Which names a wrapper exposes: an explicit collection, or `'all'` for every
 * non-underscore name. Absent (undefined) means none.
 */
export type AttrPolicy = readonly string[] | ReadonlySet<string> | 'all'

/** Options shared by both wrappers — all policies default to "expose nothing". */
export interface BaseWrapperOptions {
  /** Attributes sent into the sandbox with the instance: `'all'` sends the
   *  instance's own enumerable string-keyed props (skipping `_`-prefixed
   *  names), an explicit list reads exactly those props. */
  eagerAttrs?: AttrPolicy
  /** Attributes the sandbox may fetch on demand (prototype getters/fields
   *  included); `'all'` never exposes `_`-prefixed names. */
  lazyAttrs?: AttrPolicy
  /** Methods the sandbox may call; `'all'` exposes the functions the class
   *  defines (prototype methods, or own static functions on a `ClassType`), never
   *  `_`-prefixed names. No policy exposes `constructor`, `__proto__`,
   *  `prototype`, `arguments` or `caller`. */
  allowedMethods?: AttrPolicy
  /** Class name shown to the sandbox; defaults to the constructor name. A
   *  class-level property: on a `ClassInstance` it names the default
   *  `ClassType` materialized for the instance, so it cannot be combined
   *  with `classType` (set `name` on that wrapper instead). */
  name?: string
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

/** Options for [`ClassInstance`]. */
export interface ClassInstanceOptions extends BaseWrapperOptions {
  /** The wrapper's identity uuid (canonical 8-4-4-4-12 form, lowercased on
   *  the wrapper); defaults to a fresh uuid4 per wrapper. See [`ClassInstance.id`]. */
  id?: string
  /** A [`ClassType`] wrapper for the instance's class, overriding the default
   *  one materialized from the constructor — pass one to grant class-level
   *  policies (or a pinned class id) alongside the instance. Must wrap the
   *  instance's own constructor. Its eager class attrs are sent with every
   *  crossing of the instance, so `type(x)` in the sandbox sees them. */
  classType?: ClassType
}

/** Shared behavior of [`ClassInstance`] and [`ClassType`]: the wrapped value,
 *  the attr/method exposure policies, and the dispatch entry points the
 *  session layer calls (`getEagerAttrs`, `lookupLazyAttr`, `callMethod`). */
export abstract class BaseWrapper {
  constructor(
    /** The wrapped host object; returned unchanged when the sandbox returns the instance. */
    readonly instance: object,
    readonly options: BaseWrapperOptions = {},
  ) {
    if ((typeof instance !== 'object' && typeof instance !== 'function') || instance === null) {
      throw new TypeError('ClassInstance expects an object instance')
    }
    validatePolicy('eagerAttrs', options.eagerAttrs)
    validatePolicy('lazyAttrs', options.lazyAttrs)
    validatePolicy('allowedMethods', options.allowedMethods)
  }

  /** Class name shown to the sandbox: `options.name`, else the constructor name. */
  getName(): string {
    if (this.options.name !== undefined) {
      return this.options.name
    }
    return constructorName(this.instance)
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
   * property is absent; the sandbox then raises `AttributeError`. Properties
   * inherited from `Object.prototype` / `Function.prototype` count as absent.
   */
  lookupLazyAttr(name: string): unknown {
    if (!policyAllows(this.options.lazyAttrs, name) || !hasMember(this.instance, name)) {
      throw this.attrError(name)
    }
    return this.convertValue(name, (this.instance as Record<string, unknown>)[name])
  }

  /**
   * Calls a method on the wrapped instance for the sandbox. Throws
   * [`AttrNotExposed`] when `name` is outside `allowedMethods` or absent.
   * Under `'all'` the name must also resolve to a method the class defines
   * (see [`isMethodUnderAll`](BaseWrapper.isMethodUnderAll)); an explicit
   * list calls whatever the named property holds.
   *
   * JS functions have no keyword arguments, so a non-empty `kwargs` is
   * appended as a final options-bag argument — the way JS hosts typically
   * take named options. The return value passes through `convertValue`
   * (after settling, for a promise-returning method).
   *
   * `__call__` is always rejected on instances — only [`ClassType`] accepts
   * it (as construction) — so even `allowedMethods: 'all'` cannot invoke the
   * instance itself.
   */
  callMethod(name: string, args: unknown[], kwargs: Record<string, unknown>): unknown {
    const policy = this.options.allowedMethods
    if (name === '__call__' || !policyAllows(policy, name)) {
      throw this.attrError(name)
    }
    const owner = findMemberOwner(this.instance, name)
    if (owner === undefined) {
      throw this.attrError(name)
    }
    const method = (this.instance as Record<string, unknown>)[name]
    if (policy === 'all' && !this.isMethodUnderAll(owner, method)) {
      throw this.attrError(name)
    }
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

  /**
   * Whether `method` (owned by `owner`, an object on the wrapped value's
   * prototype chain) is a method the class defines, the only kind
   * `allowedMethods: 'all'` exposes: on an instance, a function on a
   * prototype rather than a callable stored on the instance itself. Class
   * constructors never count, so nested classes stay unreachable.
   */
  protected isMethodUnderAll(owner: object, method: unknown): boolean {
    return owner !== this.instance && isPlainFunction(method)
  }

  /** The sentinel a denied or missing attribute raises; [`ClassType`]
   *  overrides it with CPython's type-object wording. */
  protected attrError(name: string): AttrNotExposed {
    return new AttrNotExposed(attributeErrorMessage(this.getName(), name))
  }
}

/**
 * Policy wrapper exposing a host class instance to the Monty sandbox. Pass it
 * as an input or return it from an external function:
 *
 * ```ts
 * await session.feedRun('assert user.greeting() == "hi Samuel"', {
 *   inputs: { user: new ClassInstance(user, { eagerAttrs: 'all', allowedMethods: ['greeting'] }) },
 * })
 * ```
 */
export class ClassInstance extends BaseWrapper {
  /** The instance's sandbox identity: reuse one wrapper to re-send an object
   *  under the same id; reusing an id for a different object throws
   *  `TypeError`. */
  readonly id: string
  /** The [`ClassType`] wrapper carrying the class's identity and policies:
   *  `options.classType` if given, else a default one materialized from the
   *  constructor. */
  readonly classType: ClassType

  declare readonly options: ClassInstanceOptions

  constructor(instance: object, options: ClassInstanceOptions = {}) {
    super(instance, options)
    this.id = options.id === undefined ? generateUuid() : normalizeId('ClassInstance', options.id)
    const ctor = classOf(instance)
    if (ctor === undefined) {
      throw new TypeError('ClassInstance expects an instance of a class, not a null-prototype object')
    }
    if (options.classType !== undefined) {
      if (options.classType.classType !== ctor) {
        throw new TypeError("classType does not match the instance's class")
      }
      if (options.name !== undefined) {
        throw new TypeError('pass name on the ClassType wrapper, not alongside classType')
      }
      this.classType = options.classType
    } else {
      this.classType = new ClassType(ctor as new (...args: never[]) => object, { name: options.name })
    }
  }

  /** Class name shown to the sandbox: the class wrapper's, so the instance,
   *  its type object and error messages all agree. */
  override getName(): string {
    return this.classType.getName()
  }
}

/** Options for [`ClassType`]: the inherited policies applied to the class
 *  object itself (class constants, static methods), the `init` gate, and the
 *  `instance*` policies applied to every constructed instance. */
export interface ClassTypeOptions extends BaseWrapperOptions {
  /** The class's identity uuid (canonical form, lowercased on the wrapper);
   *  defaults to a process-wide id per class, so every wrapper of one class
   *  shares it. See [`ClassType.id`]. */
  id?: string
  /** Whether sandbox code may instantiate the class (default false). Purely
   *  a host-side policy: it never crosses the wire, and `construct` checks
   *  it on every request. */
  init?: boolean
  /** Policy applied to constructed instances (see [`BaseWrapperOptions`]). */
  instanceEagerAttrs?: AttrPolicy
  /** Policy applied to constructed instances. */
  instanceLazyAttrs?: AttrPolicy
  /** Policy applied to constructed instances. */
  instanceAllowedMethods?: AttrPolicy
}

/**
 * Policy wrapper exposing a host *class* to the Monty sandbox, applied to the
 * class object itself: `eagerAttrs` sends static class constants with the
 * type, `lazyAttrs` serves them on demand, and `allowedMethods` exposes
 * static methods. With `init: true`, sandbox code may call the class; the
 * construction arrives as a `__call__` method call, runs host-side, and the
 * result crosses back wrapped in a [`ClassInstance`] carrying the `instance*`
 * policies:
 *
 * ```ts
 * await session.feedRun('p = Point(1, 2)\nassert p.x == 1', {
 *   inputs: { Point: new ClassType(Point, { init: true, instanceEagerAttrs: 'all' }) },
 * })
 * ```
 */
export class ClassType extends BaseWrapper {
  /** The class's sandbox identity. Defaults to a process-wide id per class
   *  object (every wrapper of one class agrees), so instances keep a stable
   *  type identity; pass `id` to pin it explicitly, e.g. when restoring a
   *  dump in a fresh process. */
  readonly id: string

  declare readonly options: ClassTypeOptions

  constructor(classType: new (...args: never[]) => object, options: ClassTypeOptions = {}) {
    if (typeof classType !== 'function') {
      throw new TypeError('ClassType expects a class (constructor function)')
    }
    super(classType, options)
    validatePolicy('instanceEagerAttrs', options.instanceEagerAttrs)
    validatePolicy('instanceLazyAttrs', options.instanceLazyAttrs)
    validatePolicy('instanceAllowedMethods', options.instanceAllowedMethods)
    this.id = options.id === undefined ? classIdFor(classType) : normalizeId('ClassType', options.id)
  }

  /** The wrapped host class (the inherited `instance` field). */
  get classType(): new (...args: never[]) => object {
    return this.instance as new (...args: never[]) => object
  }

  /** Class name shown to the sandbox: `options.name`, else the class name. */
  override getName(): string {
    if (this.options.name !== undefined) {
      return this.options.name
    }
    const name = (this.instance as { name?: unknown }).name
    return typeof name === 'string' && name !== '' ? name : 'object'
  }

  /** Routes `__call__` (construction) to [`construct`](ClassType.construct);
   *  every other name is a static-method call gated by `allowedMethods`. */
  override callMethod(name: string, args: unknown[], kwargs: Record<string, unknown>): unknown {
    if (name === '__call__') {
      return this.construct(args, kwargs)
    }
    return super.callMethod(name, args, kwargs)
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

  /** Wraps a constructed instance with the `instance*` policies. The instance
   *  carries this wrapper as its `classType`, so its class keeps this
   *  wrapper's `id`, `name` and eager class attrs; a constructor that returns
   *  an object of another class gets that class's default wrapper instead.
   *  Override to customize how constructed instances are exposed. */
  instanceWrapper(instance: object): ClassInstance {
    const { instanceEagerAttrs, instanceLazyAttrs, instanceAllowedMethods, convertValue } = this.options
    const ownClass = classOf(instance) === this.classType
    return new ClassInstance(instance, {
      eagerAttrs: instanceEagerAttrs,
      lazyAttrs: instanceLazyAttrs,
      allowedMethods: instanceAllowedMethods,
      convertValue,
      classType: ownClass ? this : undefined,
    })
  }

  /** `'all'` on a class exposes its own static functions only: nothing
   *  inherited from a base class or `Function.prototype`. */
  protected override isMethodUnderAll(owner: object, method: unknown): boolean {
    return owner === this.instance && isPlainFunction(method)
  }

  protected override attrError(name: string): AttrNotExposed {
    return new AttrNotExposed(`type object '${this.getName()}' has no attribute '${name}'`)
  }
}

/**
 * Read-only stand-in for a class instance the host has no original object
 * for: one defined inside the sandbox, or a host instance returned after the
 * session was restored into a fresh session.
 */
export class MontyClassProxy {
  /** Class name of the instance (e.g. `'Point'`). */
  readonly name: string
  /** Whether the instance was a dataclass on the side that produced it. */
  readonly isDataclass: boolean
  /** Identity of the instance (canonical uuid string): the id the sandbox
   *  resolves the original object by when the proxy is passed back. */
  readonly id: string
  /** The instance's attributes. Null-prototype record: attr names are
   *  sandbox-controlled, so they must never become prototype properties. */
  readonly attributes: Record<string, unknown>
  /** The class as it crossed the wire, kept so the proxy can cross back. */
  private readonly classType: Record<string, unknown>

  /** @internal — built by `restore` from the wire marker. */
  constructor(classType: Record<string, unknown>, id: string, attrs: Array<[string, unknown]>) {
    this.name = typeof classType.name === 'string' ? classType.name : 'object'
    this.isDataclass = classType.isDataclass === true
    this.id = id
    this.classType = classType
    const attributes: Record<string, unknown> = Object.create(null)
    for (const [key, value] of attrs) {
      if (typeof key === 'string') {
        attributes[key] = value
      }
    }
    this.attributes = attributes
  }

  /** @internal — the wire marker `prepare` sends when the proxy is passed back
   *  into the sandbox, which hands over the original object by `id`. */
  toMarker(store: InstanceStore, depth: number): Record<string, unknown> {
    const attrs = Object.keys(this.attributes).map((key): [string, unknown] => [
      key,
      prepareInner(this.attributes[key], store, depth + 1),
    ])
    return {
      __monty_type__: 'ClassInstance',
      type: { ...this.classType, attrs: [] },
      instanceId: this.id,
      attrs,
    }
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
  /** uuid → wrapper (instances and class types — one routing namespace,
   *  since `callMethod` / `lookupLazyAttr` are the shared wrapper surface);
   *  re-sending the same wrapper overwrites its entry. */
  readonly map = new Map<string, BaseWrapper>()
  /** Registers a wrapper under its own `id` (the wrapper owns its identity).
   *  Re-sending the same wrapper (or another wrapper of the same object)
   *  overwrites the entry; an id already routing to a different object is
   *  rejected. */
  register(wrapper: ClassInstance): string {
    this.checkNoAlias(wrapper.id, wrapper.instance)
    this.map.set(wrapper.id, wrapper)
    return wrapper.id
  }

  /** Registers a `ClassType` wrapper under its class uuid for routing
   *  (method calls, `__call__` construction, lazy class attrs). Re-granting
   *  the same class with a new wrapper overwrites (last policy wins). */
  registerClass(wrapper: ClassType): string {
    this.checkNoAlias(wrapper.id, wrapper.classType)
    this.map.set(wrapper.id, wrapper)
    return wrapper.id
  }

  /** [`registerClass`](InstanceStore.registerClass), but only when the class
   *  is not yet registered — used for the `ClassType` a `ClassInstance`
   *  materializes, so an auto-built default policy never clobbers an
   *  explicitly granted one. Still rejects an id aliasing a different object. */
  registerClassIfAbsent(wrapper: ClassType): string {
    this.checkNoAlias(wrapper.id, wrapper.classType)
    if (!this.map.has(wrapper.id)) {
      this.map.set(wrapper.id, wrapper)
    }
    return wrapper.id
  }

  /** Throws if `id` is already registered for an object other than `value`
   *  (compared by identity). Two wrappers sharing an id but wrapping
   *  different objects would silently re-route method calls and round-trips
   *  from one host object to the other. */
  private checkNoAlias(id: string, value: object): void {
    const existing = this.map.get(id)
    if (existing !== undefined && existing.instance !== value) {
      throw new TypeError(`wrapper id '${id}' already identifies a different object in this session`)
    }
  }

  /** Looks up the wrapper registered for `id` (instance or class type). */
  get(id: string): BaseWrapper | undefined {
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
  constructor(message: string) {
    super(message)
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
 * `TypeError` telling the caller to wrap it.
 */
export function prepare(value: unknown, store: InstanceStore): unknown {
  return prepareInner(value, store, 0)
}

/** Recursion guard for the outbound walk itself, so a too-deep value fails
 *  with a catchable error instead of a `RangeError` mid-recursion. Not the
 *  authoritative wire budget: the native layer re-checks every value with
 *  exact per-shape accounting (`exceeds_max_value_depth`) before encoding. */
const MAX_INPUT_DEPTH = 48

function prepareInner(value: unknown, store: InstanceStore, depth: number): unknown {
  if (depth > MAX_INPUT_DEPTH) {
    throw new TypeError('Max input depth exceeded')
  }
  if (typeof value !== 'object' || value === null) {
    return value
  }
  const walk = (item: unknown) => prepareInner(item, store, depth + 1)
  // `ClassType` and `ClassInstance` are sibling `BaseWrapper`s; the class check simply comes first.
  if (value instanceof ClassType) {
    return classTypeToMarker(value, store, depth)
  }
  if (value instanceof ClassInstance) {
    return wrapperToMarker(value, store, depth)
  }
  if (value instanceof MontyClassProxy) {
    return value.toMarker(store, depth)
  }
  if (Array.isArray(value)) {
    return walkArray(value, walk)
  }
  if (value instanceof Map) {
    return walkMap(value, walk)
  }
  if (value instanceof Set) {
    return walkSet(value, walk)
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
  if (marker === 'Type' && (value as { classType?: unknown }).classType !== undefined) {
    // Same reasoning for a host-class marker; builtin `Type` markers
    // (`{ value: 'int' }`) carry no identity and pass through.
    throw new TypeError('raw Type markers are not accepted — pass the class through ClassType(...)')
  }
  if (marker !== undefined) {
    return value
  }
  if (isPlainObject(value)) {
    return walkPlainObject(value as Record<string, unknown>, walk)
  }
  throw new TypeError(
    `Cannot convert ${constructorName(value)} instance to a Monty value — wrap it in ClassInstance(...)`,
  )
}

/**
 * Inbound walk over a sandbox value reaching the host: maps `ClassInstance`
 * markers to the original wrapped object when the id is in `store` (identity
 * preserved), else to a [`MontyClassProxy`] proxy with recursively
 * restored attrs; maps a host-class `Type` marker to the registered class
 * object the same way (an unregistered class stays a marker); recurses into
 * containers. Wire values are already depth-bounded by the native layer, so
 * no guard is needed here.
 */
export function restore(value: unknown, store: InstanceStore): unknown {
  if (typeof value !== 'object' || value === null) {
    return value
  }
  const walk = (item: unknown) => restore(item, store)
  if (Array.isArray(value)) {
    return walkArray(value, walk)
  }
  if (value instanceof Map) {
    return walkMap(value, walk)
  }
  if (value instanceof Set) {
    return walkSet(value, walk)
  }
  if (value instanceof Uint8Array) {
    return value
  }
  const marker = readTypeMarker(value)
  if (marker === 'ClassInstance') {
    return markerToInstance(value as Record<string, unknown>, store)
  }
  if (marker === 'Type') {
    return markerToClass(value as Record<string, unknown>, store)
  }
  if (marker === undefined && isPlainObject(value)) {
    return walkPlainObject(value as Record<string, unknown>, walk)
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
    type: instanceTypeObject(wrapper, store, depth),
    instanceId,
    attrs,
  }
}

/** The `type` object for an instance marker, from the wrapper's [`ClassType`].
 *  Registered for routing only when the class has no wrapper yet, so an
 *  auto-materialized default never clobbers an explicit grant. The class's
 *  eager attrs cross with every instance, so the sandbox's one type object
 *  per class sees them whichever crossing arrives first. */
function instanceTypeObject(wrapper: ClassInstance, store: InstanceStore, depth: number): Record<string, unknown> {
  store.registerClassIfAbsent(wrapper.classType)
  return classTypeObject(wrapper.classType, store, depth)
}

/** Registers a `ClassType` wrapper and builds its `Type` wire marker. */
function classTypeToMarker(wrapper: ClassType, store: InstanceStore, depth: number): Record<string, unknown> {
  store.registerClass(wrapper)
  return {
    __monty_type__: 'Type',
    classType: classTypeObject(wrapper, store, depth),
  }
}

/**
 * The plain `classType` object shared by ClassInstance and Type markers:
 * name, the wrapper's uuid, and the eager class attrs (static class
 * constants), each prepared recursively so nested wrappers register too.
 */
function classTypeObject(wrapper: ClassType, store: InstanceStore, depth: number): Record<string, unknown> {
  const attrs = wrapper
    .getEagerAttrs()
    .map(([name, value]): [string, unknown] => [name, prepareInner(value, store, depth + 1)])
  return {
    name: wrapper.getName(),
    id: wrapper.id,
    hostDefined: true,
    // JS has no dataclasses; host-wrapped objects always cross as plain classes
    isDataclass: false,
    attrs,
  }
}

/** Maps an inbound `ClassInstance` marker to the original instance or a proxy. */
function markerToInstance(marker: Record<string, unknown>, store: InstanceStore): unknown {
  if (typeof marker.instanceId !== 'string') {
    throw new TypeError('ClassInstance marker instanceId must be a uuid string')
  }
  const wrapper = store.get(marker.instanceId)
  if (wrapper !== undefined) {
    return wrapper.instance
  }
  const attrs: Array<[string, unknown]> = []
  if (Array.isArray(marker.attrs)) {
    for (const pair of marker.attrs as unknown[]) {
      if (Array.isArray(pair) && typeof pair[0] === 'string') {
        attrs.push([pair[0], restore(pair[1], store)])
      }
    }
  }
  const classType = (marker.type ?? {}) as Record<string, unknown>
  return new MontyClassProxy(classType, marker.instanceId, attrs)
}

/** Maps an inbound `Type` marker to the registered host class, else leaves
 *  the marker as is (a builtin type, or a class this session never sent). */
function markerToClass(marker: Record<string, unknown>, store: InstanceStore): unknown {
  const id = (marker.classType as { id?: unknown } | undefined)?.id
  const wrapper = typeof id === 'string' ? store.get(id) : undefined
  return wrapper instanceof ClassType ? wrapper.instance : marker
}

// === shared container walks (used by both prepare and restore) ===

type Walk = (value: unknown) => unknown

/** Walks array items, preserving the `__tuple__` marker on the rebuilt array. */
function walkArray(array: unknown[], walk: Walk): unknown[] {
  const items = array.map(walk)
  if ((array as { __tuple__?: unknown }).__tuple__ === true) {
    Object.defineProperty(items, '__tuple__', { value: true, enumerable: false })
  }
  return items
}

function walkMap(map: Map<unknown, unknown>, walk: Walk): Map<unknown, unknown> {
  return new Map([...map].map(([key, value]) => [walk(key), walk(value)]))
}

function walkSet(set: Set<unknown>, walk: Walk): Set<unknown> {
  return new Set([...set].map(walk))
}

/** Walks a plain object's own enumerable entries; the rebuilt object has a
 *  null prototype so no key can land on `Object.prototype`. */
function walkPlainObject(obj: Record<string, unknown>, walk: Walk): Record<string, unknown> {
  const out: Record<string, unknown> = Object.create(null)
  for (const [key, value] of Object.entries(obj)) {
    out[key] = walk(value)
  }
  return out
}

// === identity / classification helpers ===

/** Process-wide class-id table: every wrapper of one class object agrees on
 *  its default id, so instances keep a stable type identity across sessions.
 *  Class identity is never secret (the name crosses anyway); instance ids
 *  stay per-wrapper. An explicit `id` option bypasses (and never writes)
 *  this table — that is how a dump restored in a fresh process pins ids. */
const classIds = new WeakMap<object, string>()

/** The process-wide default id for `classObject`, generated on first use. */
function classIdFor(classObject: object): string {
  let id = classIds.get(classObject)
  if (id === undefined) {
    id = generateUuid()
    classIds.set(classObject, id)
  }
  return id
}

/** Generates a canonical lowercase uuid4 string. `getRandomValues` rather than
 *  `randomUUID` so insecure browser contexts work too. */
function generateUuid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 0x0f) | 0x40 // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80 // RFC 4122 variant
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

/** Canonical 8-4-4-4-12 hex uuid; case-insensitive since `normalizeId` lowercases. */
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** Validates a caller-supplied wrapper id and lowercases it, so the id the
 *  sandbox reports back (always lowercase) is the key the store holds. */
function normalizeId(wrapperKind: string, id: string): string {
  if (typeof id !== 'string' || !UUID_PATTERN.test(id)) {
    throw new TypeError(`${wrapperKind} id must be a canonical uuid string, got ${JSON.stringify(id)}`)
  }
  return id.toLowerCase()
}

/** Rejects a string policy other than `'all'`: a bare `'greet'` would
 *  otherwise be treated as a character array (`'g'`, `'r'`, ...). */
function validatePolicy(field: string, policy: AttrPolicy | undefined): void {
  if (typeof policy === 'string' && policy !== 'all') {
    throw new TypeError(`${field} must be 'all', undefined or a list/Set of names, got '${policy}'`)
  }
}

/** Names no policy may expose, `'all'` or explicit: JS object machinery
 *  that would hand the sandbox the class, its prototype, or a call stack. */
const DENIED_NAMES: ReadonlySet<string> = new Set(['constructor', '__proto__', 'prototype', 'arguments', 'caller'])

/** Whether `policy` exposes `name`; `'all'` never exposes underscore names,
 *  and [`DENIED_NAMES`] are refused whichever form the policy takes. */
function policyAllows(policy: AttrPolicy | undefined, name: string): boolean {
  if (policy === undefined || DENIED_NAMES.has(name)) {
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

/** Prototypes every object or function ends in: members found there
 *  (`toString`, `hasOwnProperty`, `call`, `bind`, ...) are never exposed. */
const PROTOTYPE_ROOTS: ReadonlySet<object> = new Set([Object.prototype, Function.prototype])

/** The object on `target`'s prototype chain (stopping before the shared
 *  roots) that owns property `name`, or `undefined` when none does. */
function findMemberOwner(target: object, name: string): object | undefined {
  for (let obj: object | null = target; obj !== null && !PROTOTYPE_ROOTS.has(obj); obj = Object.getPrototypeOf(obj)) {
    if (Object.prototype.hasOwnProperty.call(obj, name)) {
      return obj
    }
  }
  return undefined
}

/** Whether `target` has `name` below the shared prototype roots; the
 *  replacement for `name in target`, which reaches `Object.prototype`. */
function hasMember(target: object, name: string): boolean {
  return findMemberOwner(target, name) !== undefined
}

/** A callable that is not a class constructor. Class constructors carry a
 *  non-writable `prototype` (ordinary functions a writable one; arrows and
 *  methods none), which is how the spec distinguishes them. */
function isPlainFunction(value: unknown): boolean {
  if (typeof value !== 'function') {
    return false
  }
  const prototype = Object.getOwnPropertyDescriptor(value, 'prototype')
  return prototype === undefined || prototype.writable === true
}

/** True when the object's prototype is `Object.prototype` or `null`. */
function isPlainObject(value: object): boolean {
  const proto: unknown = Object.getPrototypeOf(value)
  return proto === Object.prototype || proto === null
}

/** Reads a value's `__monty_type__` marker, if it has one. */
function readTypeMarker(value: object): string | undefined {
  const marker = (value as { __monty_type__?: unknown }).__monty_type__
  return typeof marker === 'string' ? marker : undefined
}

/** The class an object is an instance of, read from its prototype so an own
 *  `constructor` property cannot spoof it; `undefined` for a null-prototype
 *  object. */
function classOf(value: object): Function | undefined {
  const proto = Object.getPrototypeOf(value) as { constructor?: unknown } | null
  const ctor = proto?.constructor
  return typeof ctor === 'function' ? ctor : undefined
}

/** The class name of an object (see [`classOf`]), `'object'` when it has none. */
function constructorName(value: object): string {
  const name = classOf(value)?.name
  return typeof name === 'string' && name !== '' ? name : 'object'
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return typeof value === 'object' && value !== null && typeof (value as { then?: unknown }).then === 'function'
}
