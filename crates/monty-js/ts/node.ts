// Node-specific capabilities extend the shared public API.
export * from './index.js'
export { type MountDirMode, type MountDirOptions } from './mount.js'
export { MountDir } from './mountDir.js'
export { findMontyBinary } from './binary.js'
export {
  flushTelemetry,
  instrumentTelemetry,
  MontyInstrumentation,
  type MontyInstrumentationConfig,
  type TelemetryComponents,
} from './telemetry.js'
