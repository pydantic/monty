// The contract `WorkerTransport` drives a session's worker through: a local
// wasm component behind a worker channel, or a remote worker over a WebSocket.

import type { CrashedTurn, DisconnectedTurn, ProtocolTurn } from '../native.js'
import type {
  DispatchResult as ComponentDispatchResult,
  Request as ComponentRequest,
} from './component/monty.component.js'

/** Semantic request accepted by the Rust component. */
export type DispatchRequest = ComponentRequest

/** Component output plus what the host side adds: termination diagnostics, and
 *  the session ID a remote that stores sessions names. */
export interface DispatchResult extends ComponentDispatchResult {
  exitStatus?: string | null
  sessionId?: Uint8Array
}

/**
 * Sends one semantic request, optionally bounded by a duration backstop. A
 * dispatcher that can stream prints hands each to `onPrint` as it arrives and
 * leaves it out of the result; one that cannot returns them all in `events`.
 */
export type Dispatcher = (
  request: DispatchRequest,
  timeoutMs?: number,
  onPrint?: (stream: 'stdout' | 'stderr', text: string) => void,
) => Promise<DispatchResult>

/** Thrown by a dispatcher that lost its worker, carrying the turn the session ends with. */
export class DispatchFailure extends Error {
  constructor(readonly turn: CrashedTurn | DisconnectedTurn | ProtocolTurn) {
    super(turn.message)
    this.name = 'DispatchFailure'
  }
}
