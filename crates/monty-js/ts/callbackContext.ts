// Node installs these handlers; browser transports use the direct callback path.

type NativePrintCallback = (stream: 'stdout' | 'stderr', text: string, parent?: string | null) => void

interface CallbackContextHandlers {
  bindPrint(callback: NativePrintCallback): NativePrintCallback
  run<T>(parent: string | undefined, callback: () => T): T
}

let handlers: CallbackContextHandlers | undefined

export function setCallbackContextHandlers(value: CallbackContextHandlers): void {
  handlers = value
}

export function bindPrintCallback(callback: NativePrintCallback): NativePrintCallback {
  return handlers?.bindPrint(callback) ?? callback
}

export function runWithCallbackContext<T>(parent: string | undefined, callback: () => T): T {
  return handlers === undefined ? callback() : handlers.run(parent, callback)
}
