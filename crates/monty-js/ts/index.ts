// Native pool plus the public API shared with WASM.
export { Monty, type MontyOptions } from './pool.js'
export {
  MontyRemote,
  type MontyRemoteOptions,
  type RemoteCheckoutOptions,
  // the remote pool's former names, kept as plain aliases
  MontyRemote as MontyWebsocket,
  type MontyRemoteOptions as MontyWebsocketOptions,
  type RemoteCheckoutOptions as WebsocketCheckoutOptions,
} from './remote.js'
export * from './shared.js'
