// Shared public API of @pydantic/monty. Node resolves this package to the
// native subprocess backend; browser bundlers resolve it to the wasm Worker
// backend. Environment-specific APIs live under @pydantic/monty/node and
// @pydantic/monty/wasm.

export { Monty, type CheckoutOptions, type MontyOptions, type ResourceLimits } from './pool.js'
export {
  ClassInstance,
  ClassType,
  MontyClassProxy,
  type AttrPolicy,
  type BaseWrapperOptions,
  type ClassInstanceOptions,
  type ClassTypeOptions,
} from './classInstance.js'
export { type AssertMessageAnnotations, type TypeCheckFormat } from './options.js'
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
  MontyFileHandle,
  type MontyFileHandleOptions,
  type MontyTime,
  type MontyTimeDelta,
  type MontyTimeZone,
} from './types.js'
export { MAX_VALUE_DEPTH } from '../native-addon.js'
