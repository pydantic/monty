// A host module as `externalModules` takes it, and as a module value crossing
// back out of the sandbox restores to.

import { generateUuid, normalizeId } from './uuid.js'

/**
 * An importable host module: its attributes, an optional `.pyi` stub for type
 * checking, and its identity. Supply it in [`CheckoutOptions.externalModules`],
 * or as another module's attribute for a submodule such as `pkg.sub`.
 * `attrs` is copied and checked here, so a module is immutable once made.
 */
export class ExternalModule {
  /** The module's public attributes: a frozen copy of the object given. */
  readonly attrs: Readonly<Record<string, unknown>>
  /** The object given, the receiver of the module's functions.
   *  @internal */
  readonly receiver: Record<string, unknown>
  /** `.pyi` source for imports, including package layout for submodules.
   *  Required for type checking unless the checker already provides the module. */
  readonly stubs: string | undefined
  /** The module's identity (canonical lowercase uuid), fresh unless given. */
  readonly id: string

  constructor(attrs: Record<string, unknown>, options: { stubs?: string; id?: string } = {}) {
    const proto = attrs !== null && typeof attrs === 'object' ? Object.getPrototypeOf(attrs) : undefined
    if (proto !== Object.prototype && proto !== null) {
      throw new TypeError(`ExternalModule attrs must be a plain object, got ${kindOf(attrs)}`)
    }
    // a null prototype, so a key like `__proto__` is an own attribute
    const copy: Record<string, unknown> = Object.create(null) as Record<string, unknown>
    // filtered before any value is read, so a private getter never runs
    for (const key of Object.keys(attrs).filter((key) => !key.startsWith('_'))) {
      // each key is one component of a host function's dotted path, so a keyword cannot be one
      if (!IDENTIFIER.test(key) || KEYWORDS.has(key)) {
        throw new TypeError(`ExternalModule attrs key '${key}' is not a valid identifier`)
      }
      const value = attrs[key]
      if (value instanceof ExternalModule) {
        // a submodule is imported by its key, so that needs to be a module name too
        checkModuleName(`ExternalModule attrs.${key}`, key)
      }
      copy[key] = value
    }
    if (options.stubs !== undefined && typeof options.stubs !== 'string') {
      throw new TypeError(`ExternalModule stubs must be a string, got ${kindOf(options.stubs)}`)
    }
    this.attrs = Object.freeze(copy)
    this.receiver = attrs
    this.stubs = options.stubs
    this.id = options.id === undefined ? generateUuid() : normalizeId('ExternalModule', options.id)
  }
}

/** Rejects a name a module cannot be imported by: a non-identifier, a keyword
 *  or a module the sandbox provides itself. `what` names the offending entry. */
export function checkModuleName(what: string, name: string): void {
  if (!IDENTIFIER.test(name) || KEYWORDS.has(name)) {
    throw new TypeError(`${what} is not a valid module name`)
  }
  if (SANDBOX_MODULES.has(name)) {
    throw new TypeError(`${what} is provided by the sandbox and cannot be replaced`)
  }
}

/** How a `TypeError` names a value's kind: `null`, a primitive's type, or its class. */
export function kindOf(value: unknown): string {
  return value === null
    ? 'null'
    : typeof value !== 'object'
      ? typeof value
      : `a ${(value as { constructor?: { name?: string } }).constructor?.name ?? 'object'}`
}

/** Python identifier syntax (XID classes, as the parser's) for module and attribute names. */
const IDENTIFIER = /^[\p{XID_Start}_][\p{XID_Continue}]*$/u

/** Keywords pass the identifier regex but cannot name modules. */
const KEYWORDS = new Set([
  'False',
  'None',
  'True',
  'and',
  'as',
  'assert',
  'async',
  'await',
  'break',
  'class',
  'continue',
  'def',
  'del',
  'elif',
  'else',
  'except',
  'finally',
  'for',
  'from',
  'global',
  'if',
  'import',
  'in',
  'is',
  'lambda',
  'nonlocal',
  'not',
  'or',
  'pass',
  'raise',
  'return',
  'try',
  'while',
  'with',
  'yield',
])

/** Keep in sync with `monty-types::SANDBOX_MODULE_NAMES`; these imports never reach the host. */
const SANDBOX_MODULES = new Set([
  'asyncio',
  'base64',
  'binascii',
  'collections',
  'copy',
  'dataclasses',
  'datetime',
  'functools',
  'gc',
  'hashlib',
  'itertools',
  'json',
  'math',
  'os',
  'pathlib',
  'posixpath',
  'random',
  're',
  'sys',
  'time',
  'typing',
  'unicodedata',
])
