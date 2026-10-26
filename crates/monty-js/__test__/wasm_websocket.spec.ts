// The WebAssembly `MontyWebsocket` end-to-end, in Node and in the browser,
// against servers that bridge each connection to a real `monty subprocess`
// child (started by websocket_setup.ts). Mirrors node_websocket.spec.ts, which
// covers the native client.

import { inject, test } from 'vitest'
import { t } from './assertions.js'

import {
  MontyCrashedError,
  MontyDisconnectError,
  MontyRuntimeError,
  MontyShutdown,
  MontyWebsocket,
  ProtocolError,
} from '@pydantic/monty/wasm'

const servers = inject('websocketServers')

/** A session or shutdown ID as text; IDs are bytes on the wire. */
function text(bytes: Uint8Array | null | undefined): string | undefined {
  return bytes === undefined || bytes === null ? undefined : new TextDecoder().decode(bytes)
}

test('feedRun over a websocket keeps session state', async () => {
  await using pool = await MontyWebsocket.create(servers.relay)
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
  await session.feedRun('x = 21')
  t.is(await session.feedRun('x * 2'), 42)
})

test('inputs, prints and async external functions cross the websocket', async () => {
  const printed: string[] = []
  await using pool = await MontyWebsocket.create(servers.relay)
  await using session = await pool.checkout()
  const result = await session.feedRun("print('hi')\nawait double(n) + 1", {
    inputs: { n: 20 },
    externalLookup: { double: async (x: number) => x * 2 },
    printCallback: (stream, text) => printed.push(`${stream}:${text}`),
  })
  t.is(result, 41)
  t.deepEqual(printed, ['stdout:hi\n'])
})

test('a runtime error keeps the session', async () => {
  await using pool = await MontyWebsocket.create(servers.relay)
  await using session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('1 / 0'), { instanceOf: MontyRuntimeError })
  t.is(error.display('type-msg'), 'ZeroDivisionError: division by zero')
  t.is(await session.feedRun('2 + 2'), 4)
})

test('separate checkouts are isolated', async () => {
  await using pool = await MontyWebsocket.create(servers.relay)
  {
    await using session = await pool.checkout()
    await session.feedRun('leaked = 1')
  }
  await using session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('leaked'), { instanceOf: MontyRuntimeError })
  t.is(error.display('msg'), "name 'leaked' is not defined")
})

test('the suspension limit is enforced on the client', async () => {
  await using pool = await MontyWebsocket.create(servers.relay)
  await using session = await pool.checkout({ limits: { maxSuspensions: 2 } })
  const error = await t.throwsAsync(() => session.feedRun('f()\nf()\nf()', { externalLookup: { f: () => null } }), {
    instanceOf: MontyRuntimeError,
  })
  t.is(error.display('type-msg'), 'RuntimeError: suspension limit 2 exceeded')
})

test('a refused connection rejects the checkout', async () => {
  await using pool = await MontyWebsocket.create('ws://127.0.0.1:9')
  await t.throwsAsync(() => pool.checkout(), {
    message: 'failed to connect to monty worker at ws://127.0.0.1:9: the connection or upgrade was refused',
  })
})

test('a server that never answers trips the request timeout', async () => {
  await using pool = await MontyWebsocket.create(servers.silent, { requestTimeout: 0.2 })
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: MontyCrashedError })
  t.true(error.timedOut)
  t.is(error.message, 'RuntimeError: monty worker killed after exceeding request timeout of 200ms')
})

test('a frame that does not decode is a protocol error', async () => {
  await using pool = await MontyWebsocket.create(servers.garbage)
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: ProtocolError })
  t.is(
    error.message,
    'invalid payload from worker: frame decode error: failed to decode Protobuf message: invalid varint',
  )
})

test('a plain relay names no session, whatever ephemeral and profile say', async () => {
  await using pool = await MontyWebsocket.create(servers.relay)
  for (const options of [{}, { ephemeral: true }, { ephemeral: false }, { profile: 'gpu' }, { profile: '' }]) {
    await using session = await pool.checkout(options)
    t.is(session.sessionId, undefined)
    t.is(await session.feedRun('1 + 1'), 2)
  }
})

test('dump bytes from a plain relay load into a new session', async () => {
  await using pool = await MontyWebsocket.create(servers.relay)
  let dump: Uint8Array
  {
    await using session = await pool.checkout()
    await session.feedRun('x = 20')
    dump = await session.dump()
  }
  await using session = await pool.checkout()
  await session.loadSession(dump)
  t.is(session.sessionId, undefined)
  t.is(await session.feedRun('x + 1'), 21)
})

test('a dropped connection raises MontyDisconnectError and loses the session', async () => {
  await using pool = await MontyWebsocket.create(servers.dropThird)
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
  const error = await t.throwsAsync(() => session.feedRun('2 + 2'), { instanceOf: MontyDisconnectError })
  t.is(error.message, 'RuntimeError: monty worker connection closed while waiting for a reply')
  t.is(await t.throwsAsync(() => session.feedRun('3 + 3')), error)
})

test('a dropped connection during dump raises MontyDisconnectError', async () => {
  await using pool = await MontyWebsocket.create(servers.dropSecond)
  await using session = await pool.checkout()
  await t.throwsAsync(() => session.dump(), { instanceOf: MontyDisconnectError })
})

test('a storing server names the session and a shutdown is resumed', async () => {
  await using pool = await MontyWebsocket.create(servers.storing)
  const session = await pool.checkout()
  t.is(text(session.sessionId), 'sess-1')
  await session.feedRun('x = 20')
  // the fake parks the session instead of running this feed; the pool
  // reloads the ID on a new connection and re-sends it
  t.is(await session.feedRun('x + 1'), 21)
  await session.close()
  // the ID the resume adopted outlives the checkout
  t.is(text(session.sessionId), 'sess-3')
})

test('with autoResume off a shutdown raises MontyShutdown naming the session', async () => {
  await using pool = await MontyWebsocket.create(servers.storingNoResume, { autoResume: false })
  {
    await using session = await pool.checkout()
    await session.feedRun('x = 20')
    const error = await t.throwsAsync(() => session.feedRun('x + 1'), { instanceOf: MontyShutdown })
    t.is(
      error.message,
      'RuntimeError: monty server is shutting down; the request did not run (restorable state attached)',
    )
    t.is(text(error.dump), 'sess-1')
  }
  await using session = await pool.checkout()
  await session.loadSession(new TextEncoder().encode('sess-1'))
  t.is(text(session.sessionId), 'sess-3')
  t.is(await session.feedRun('x + 1'), 21)
})

test('a closed pool rejects checkouts', async () => {
  const pool = await MontyWebsocket.create(servers.relay)
  await pool.close()
  await t.throwsAsync(() => pool.checkout(), {
    message: 'the pool is closed — create a new MontyWebsocket pool',
  })
})

test('options a browser cannot honour are rejected', async () => {
  const connectHeaders = () => ({ 'x-token': 't' })
  await t.throwsAsync(() => MontyWebsocket.create(servers.relay, { connectHeaders } as never), {
    instanceOf: TypeError,
    message:
      'connectHeaders is not supported by the WebAssembly MontyWebsocket: a browser WebSocket cannot set upgrade headers',
  })
  await t.throwsAsync(() => MontyWebsocket.create(servers.relay, { maxProcesses: 2 } as never), {
    instanceOf: TypeError,
    message:
      'maxProcesses is not supported by the WebAssembly MontyWebsocket: it has no connection cap; each checkout dials at once',
  })
  await t.throwsAsync(() => MontyWebsocket.create(servers.relay, { checkoutTimeout: 1 } as never), {
    instanceOf: TypeError,
    message:
      'checkoutTimeout is not supported by the WebAssembly MontyWebsocket: it has no connection cap; each checkout dials at once',
  })
  // unset, as options shared with the native client leave them
  const shared = { connectHeaders: undefined, maxProcesses: undefined, checkoutTimeout: undefined }
  await using pool = await MontyWebsocket.create(servers.relay, shared as never)
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
})

test('only ws:// and wss:// URLs are accepted', async () => {
  for (const url of ['http://127.0.0.1:8000/', 'not a url']) {
    await t.throwsAsync(() => MontyWebsocket.create(url), {
      instanceOf: TypeError,
      message: `MontyWebsocket url must be a ws:// or wss:// URL, got ${JSON.stringify(url)}`,
    })
  }
})
