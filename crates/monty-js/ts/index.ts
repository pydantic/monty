// Shared public API of @pydantic/monty. Node resolves this package to the
// native subprocess backend; browser bundlers resolve it to the wasm Worker
// backend. Environment-specific APIs live under @pydantic/monty/node and
// @pydantic/monty/wasm.

export { Monty, type CheckoutOptions, type MontyOptions, type ResourceLimits } from './pool.js'
export { type AssertMessageAnnotations } from './options.js'
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
  type PrintTargetInput,
  type Snapshot,
} from './session.js'
export { CollectString, CollectStreams, DEFAULT_MAX_PRINT_COLLECT_BYTES, type CollectedStreamEntry } from './print.js'
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
export { MAX_VALUE_DEPTH } from '../native-addon.js'
