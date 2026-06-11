// Public API of @pydantic/monty: a pool of crash-isolated `monty`
// subprocess workers (`Monty`), sessions checked out of it (`MontySession`),
// filesystem mounts, and the error hierarchy. The interpreter itself always
// runs in worker subprocesses — a sandbox crash can never take down the
// host process.

export { Monty, type CheckoutOptions, type MontyOptions, type ResourceLimits } from './pool.js'
export {
  MontySession,
  NOT_HANDLED,
  type ExternalFunction,
  type FeedOptions,
  type OsCallback,
  type PrintCallback,
} from './session.js'
export { MountDir, type MountDirMode, type MountDirOptions } from './mount.js'
export {
  MontyCrashedError,
  MontyError,
  MontyRuntimeError,
  MontySyntaxError,
  MontyTypingError,
  type ExceptionInfo,
  type Frame,
} from './errors.js'
export {
  ConversionError,
  MAX_VALUE_DEPTH,
  type MontyDate,
  type MontyDateTime,
  type MontyException,
  type MontyFileHandle,
  type MontyTimeDelta,
  type MontyTimeZone,
} from './convert.js'
export { ProtocolError } from './worker.js'
export { findMontyBinary } from './binary.js'
