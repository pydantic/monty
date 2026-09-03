export declare const MAX_VALUE_DEPTH: number

export declare function _installTelemetryAdapter(version: number, callback: (event: string) => void): void

export interface NativeMount {
  virtualPath: string
  hostPath: string
  mode?: string
  writeBytesLimit?: number
  memoryUsageLimit: number
}

export declare class NativeMountDir {
  constructor(mount: NativeMount)
  close(): void
}

export declare class NativePool {
  constructor(options?: unknown)
  start(): Promise<void>
  checkout(options?: unknown): NativeSession
  close(): Promise<void>
}

export declare class NativeSession {
  readonly workerPid?: number
  enter(telemetryContext?: unknown): Promise<void>
  feed(...args: unknown[]): Promise<object>
  feedStart(...args: unknown[]): Promise<object>
  restore(...args: unknown[]): Promise<object>
  dump(): Promise<Uint8Array>
  finish(): Promise<void>
  installDependencies(...args: unknown[]): Promise<object>
  resumeReturn(...args: unknown[]): Promise<object>
  resumeError(...args: unknown[]): Promise<object>
  resumeNotFound(...args: unknown[]): Promise<object>
  resumeNotHandled(...args: unknown[]): Promise<object>
  resumeFromMounts(...args: unknown[]): Promise<object>
  resumeFuture(...args: unknown[]): Promise<object>
  resumeNameLookup(...args: unknown[]): Promise<object>
  resumeLazyAttr(...args: unknown[]): Promise<object>
  resumeNameLookupError(...args: unknown[]): Promise<object>
  resolveFutures(...args: unknown[]): Promise<object>
}
