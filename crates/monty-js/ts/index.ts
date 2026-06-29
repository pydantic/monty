// Public API of @pydantic/monty: a pool of crash-isolated `monty`
// subprocess workers (`Monty`), sessions checked out of it (`MontySession`),
// filesystem mounts, and the error hierarchy. The interpreter runs in worker
// subprocesses via the native `monty-pool` binding — a sandbox crash can
// never take down the host process.
//
// The legacy in-process API (the only option on wasm/browsers, where
// subprocesses do not exist) is exposed separately via the
// `@pydantic/monty/wasm` subpath.

export { Monty, type CheckoutOptions, type MontyOptions, type ResourceLimits } from './pool.js'
export {
  FunctionSnapshot,
  FutureSnapshot,
  MontyComplete,
  MontySession,
  NameLookupSnapshot,
  NOT_HANDLED,
  type ExternalFunction,
  type FeedOptions,
  type FeedStartOptions,
  type FutureResolution,
  type LoadSnapshotOptions,
  type OsCallback,
  type PrintCallback,
  type Snapshot,
} from './session.js'
export { MountDir, type MountDirMode, type MountDirOptions } from './mount.js'
export {
  MontyCrashedError,
  MontyError,
  MontyRuntimeError,
  MontySyntaxError,
  MontyTypingError,
  ProtocolError,
  type ExceptionInfo,
  type Frame,
} from './errors.js'
export {
  type MontyDate,
  type MontyDateTime,
  type MontyException,
  type MontyFileHandle,
  type MontyTimeDelta,
  type MontyTimeZone,
} from './types.js'
export { findMontyBinary } from './binary.js'
export { MAX_VALUE_DEPTH } from '../index.js'
