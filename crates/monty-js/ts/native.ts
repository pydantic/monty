// Hand-written typings for the turn objects produced by the native
// `NativeSession` methods (src/pool.rs). The napi-generated native-addon d.ts
// types each turn as a plain `object`; these interfaces are the contract the
// Rust side upholds — every turn-producing method resolves to exactly one of
// the `NativeTurn` variants below.

/** One frame of a Python traceback, as shipped by the native binding. */
export interface NativeFrame {
  filename: string
  line: number
  column: number
  endLine: number
  endColumn: number
  frameName?: string
  previewLine?: string
  hideCaret: boolean
  hideFrameName: boolean
}

/** A sandbox exception: type name, message, the worker-rendered Python
 *  traceback string, and the structured frames behind it. */
export interface NativeException {
  excType: string
  message: string
  /** Full Python traceback, rendered by the worker (monty's `MontyException`
   *  Display). Used verbatim; never re-rendered in TypeScript. */
  traceback: string
  frames: NativeFrame[]
}

/** The fed snippet completed with this (already converted) value. */
export interface CompleteTurn {
  kind: 'complete'
  value: unknown
}

/** The sandbox called an external function — answer with a `resume*` call. */
export interface FunctionCallTurn {
  kind: 'functionCall'
  functionName: string
  /** Positional arguments, already converted to JS values. */
  args: unknown[]
  /**
   * Keyword arguments as `[key, value]` pairs. Keys are values (usually
   * strings), never object properties — build records with a null prototype.
   */
  kwargs: [unknown, unknown][]
  callId: number
  methodCall: boolean
}

/** The sandbox performed an OS operation no mount handled. */
export interface OsCallTurn {
  kind: 'osCall'
  functionName: string
  args: unknown[]
  kwargs: [unknown, unknown][]
  callId: number
}

/** The sandbox read an undefined name — answer with `resumeNameLookup`. */
export interface NameLookupTurn {
  kind: 'nameLookup'
  name: string
}

/** Every sandbox task is blocked on external futures. */
export interface ResolveFuturesTurn {
  kind: 'resolveFutures'
  pendingCallIds: number[]
}

/** The sandbox raised; the worker and session stay usable. */
export interface ErrorTurn {
  kind: 'error'
  exception: NativeException
}

/** Type checking rejected the snippet; the worker and session stay usable. */
export interface TypingErrorTurn {
  kind: 'typingError'
  diagnostics: string
}

/** The worker died (crash or watchdog kill); the session is lost. */
export interface CrashedTurn {
  kind: 'crashed'
  message: string
  timedOut: boolean
  exitStatus?: string
}

/** The worker (or caller) violated the protocol; the session is lost. */
export interface ProtocolTurn {
  kind: 'protocol'
  message: string
}

/** A restore of an idle (between-feeds) dump — no suspension to resume. Only
 *  produced by `NativeSession.restore`, surfaced by `MontySession.loadSession`. */
export interface LoadedTurn {
  kind: 'loaded'
}

/** A non-feed request succeeded with no value or suspension. Only produced by
 *  `NativeSession.installDependencies`. */
export interface OkTurn {
  kind: 'ok'
}

/** No mount covered the pending OS call, which stays suspended for the caller
 *  to answer. Only produced by `NativeSession.resumeFromMounts`. */
export interface NotMountedTurn {
  kind: 'notMounted'
}

/** Everything one protocol turn can resolve to. */
export type NativeTurn =
  | CompleteTurn
  | FunctionCallTurn
  | OsCallTurn
  | NameLookupTurn
  | ResolveFuturesTurn
  | ErrorTurn
  | TypingErrorTurn
  | CrashedTurn
  | ProtocolTurn

/** One settled promise outcome delivered to `resolveFutures`. */
export interface NativeFutureResult {
  callId: number
  ok: boolean
  value?: unknown
  excType?: string
  message?: string
}
