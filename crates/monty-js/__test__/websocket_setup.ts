// Vitest global setup for the WebAssembly `MontyWebsocket` specs: the servers
// run here, in Node, so a browser spec can dial them too. Each scenario gets
// its own server, since a storing server's behaviour depends on how many
// connections it has seen.

import type { TestProject } from 'vitest/node'

import { droppingRelay, garbage, relay, serve, silent, StoringServer, type TestServer } from './websocket_servers.js'

/** The URL of each scenario's server. */
export interface WebsocketServers {
  relay: string
  /** Drops each connection, unanswered, at its third request (the first is Configure). */
  dropThird: string
  /** Drops each connection, unanswered, at its second request. */
  dropSecond: string
  silent: string
  garbage: string
  /** A storing server, one per spec that drains a session. */
  storing: string
  storingNoResume: string
}

declare module 'vitest' {
  export interface ProvidedContext {
    websocketServers: WebsocketServers
  }
}

export default async function setup(project: TestProject): Promise<() => Promise<void>> {
  const started: Record<keyof WebsocketServers, TestServer> = {
    relay: await serve(relay),
    dropThird: await serve(droppingRelay(3)),
    dropSecond: await serve(droppingRelay(2)),
    silent: await serve(silent),
    garbage: await serve(garbage),
    storing: await serve(new StoringServer().handle),
    storingNoResume: await serve(new StoringServer().handle),
  }
  project.provide(
    'websocketServers',
    Object.fromEntries(
      Object.entries(started).map(([name, server]) => [name, server.url]),
    ) as unknown as WebsocketServers,
  )
  return async () => {
    await Promise.all(Object.values(started).map((server) => server.close()))
  }
}
