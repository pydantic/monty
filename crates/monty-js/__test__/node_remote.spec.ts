// `MontyRemote` end-to-end against in-process gRPC servers that bridge each
// session stream to a real `monty subprocess` child (see remote_servers.ts),
// mirroring crates/monty-python/tests/test_remote.py.

import { AsyncLocalStorage } from 'node:async_hooks'

import { status } from '@grpc/grpc-js'
import { afterEach, test } from 'vitest'
import { t } from './assertions.js'
import {
  admit,
  drainingServer,
  droppingRelay,
  endWithStatus,
  messages,
  OK_EVENT,
  PING_EVENT,
  protoFields,
  refusingServer,
  relay,
  serve,
  StoringServer,
  type TestServer,
} from './remote_servers.js'

import {
  MontyDisconnectError,
  MontyRefusedError,
  MontyRemote,
  type MontyRemoteOptions,
  MontyRuntimeError,
  MontyShutdown,
  MontyWebsocket,
  type MontyWebsocketOptions,
} from '@pydantic/monty'

const servers: TestServer[] = []

/** Serves `handle` until the end of the test. */
async function server(handle: Parameters<typeof serve>[0]): Promise<TestServer> {
  const started = await serve(handle)
  servers.push(started)
  return started
}

afterEach(async () => {
  await Promise.all(servers.splice(0).map((s) => s.close()))
})

test('feedRun over gRPC keeps session state', async () => {
  const { url } = await server(relay)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
  await session.feedRun('x = 21')
  t.is(await session.feedRun('x * 2'), 42)
})

test('inputs, prints and async external functions cross the stream', async () => {
  const { url } = await server(relay)
  const printed: string[] = []
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  const result = await session.feedRun("print('hi')\nawait double(n) + 1", {
    inputs: { n: 20 },
    externalLookup: { double: async (x: number) => x * 2 },
    printCallback: (stream, text) => printed.push(`${stream}:${text}`),
  })
  t.is(result, 41)
  t.deepEqual(printed, ['stdout:hi\n'])
})

test('calls to async functions start without suspending the remote worker', async () => {
  const { url } = await server(relay)
  const log: string[] = []
  const fetch = async (x: number) => {
    log.push(`fetch(${x})`)
    return x * 10
  }
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  const code = "import asyncio\naws = [fetch(i) for i in range(3)]\nprint('started')\nawait asyncio.gather(*aws)"
  const result = await session.feedRun(code, {
    externalLookup: { fetch },
    printCallback: (_stream, text) => log.push(text),
  })
  t.deepEqual(result, [0, 10, 20])
  // every call started on the host; a call start and a print run on separate
  // tasks, so their relative order is not part of the contract
  t.deepEqual([...log].sort(), ['fetch(0)', 'fetch(1)', 'fetch(2)', 'started\n'])
})

test('separate checkouts are isolated', async () => {
  const { url } = await server(relay)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  {
    await using session = await pool.checkout()
    await session.feedRun('leaked = 1')
  }
  await using session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('leaked'), { instanceOf: MontyRuntimeError })
  t.is(error.display('msg'), "name 'leaked' is not defined")
})

test('metadata runs per checkout in the caller async context', async () => {
  const relayServer = await server(relay)
  const trace = new AsyncLocalStorage<string>()
  let calls = 0
  await using pool = await MontyRemote.create(relayServer.url, {
    requestTimeout: 30,
    metadata: async () => {
      calls += 1
      return { traceparent: trace.getStore() ?? 'missing', 'x-token': 't' }
    },
  })
  const checkoutAs = (id: string) =>
    trace.run(id, async () => {
      await using session = await pool.checkout()
      // Await the feed before leaving the scope that disposes the session.
      return await session.feedRun('1 + 1')
    })
  t.deepEqual(await Promise.all([checkoutAs('00-aaa-111-01'), checkoutAs('00-bbb-222-01')]), [2, 2])
  t.is(calls, 2)
  t.deepEqual(relayServer.headers.map((headers) => headers.traceparent).sort(), ['00-aaa-111-01', '00-bbb-222-01'])
  t.deepEqual(
    relayServer.headers.map((headers) => headers['x-token']),
    ['t', 't'],
  )
})

test('metadata must be a function', async () => {
  const notCallable = { 'x-token': 't' } as unknown as () => Record<string, string>
  const cases: Array<[MontyRemoteOptions, string]> = [
    [{ metadata: notCallable }, 'metadata'],
    // an explicit null is a bad value, not a missing one the alias fills in
    [{ metadata: null as unknown as () => Record<string, string> }, 'metadata'],
    // the deprecated alias names itself, the option the caller passed
    [{ connectHeaders: notCallable }, 'connectHeaders'],
  ]
  for (const [options, option] of cases) {
    await t.throwsAsync(() => MontyRemote.create('http://127.0.0.1:9', options), {
      instanceOf: TypeError,
      message: `${option} must be a function returning a record of header names to values`,
    })
  }
})

test('connectHeaders is a deprecated alias of metadata', async () => {
  const relayServer = await server(relay)
  await using pool = await MontyRemote.create(relayServer.url, {
    requestTimeout: 30,
    connectHeaders: () => ({ 'x-token': 'legacy' }),
  })
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
  t.deepEqual(
    relayServer.headers.map((headers) => headers['x-token']),
    ['legacy'],
  )
  await t.throwsAsync(
    () =>
      MontyRemote.create(relayServer.url, {
        metadata: () => ({}),
        connectHeaders: () => ({}),
      }),
    { instanceOf: TypeError, message: 'pass metadata, not connectHeaders as well' },
  )
})

test('a bad metadata result rejects the checkout and leaves the pool usable', async () => {
  const relayServer = await server(relay)
  let result: unknown = 5
  await using pool = await MontyRemote.create(relayServer.url, {
    requestTimeout: 30,
    metadata: () => result as Record<string, string>,
  })
  await t.throwsAsync(() => pool.checkout(), {
    instanceOf: TypeError,
    message: 'metadata must return a record of header names to values, got number',
  })
  result = { 'x-token': 1 }
  await t.throwsAsync(() => pool.checkout(), {
    instanceOf: TypeError,
    message: "metadata must return string header values, got number for 'x-token'",
  })
  result = { 'x-token': 't' }
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
  t.is(relayServer.headers.length, 1)
})

test('a refused connection rejects the checkout', async () => {
  await using pool = await MontyRemote.create('http://127.0.0.1:9', { requestTimeout: 30 })
  const error = await t.throwsAsync(() => pool.checkout())
  t.regex(error.message, /^failed to spawn monty worker: http:\/\/127\.0\.0\.1:9/)
})

test('a session the server refuses rejects the checkout with MontyRuntimeError', async () => {
  const { url } = await server(refusingServer("unknown profile 'gpu'"))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  const error = await t.throwsAsync(() => pool.checkout({ profile: 'gpu' }), { instanceOf: MontyRuntimeError })
  t.is(error.message, "RuntimeError: unknown profile 'gpu'")
})

test('a connection dropped during checkout rejects it with MontyDisconnectError', async () => {
  const { url } = await server(droppingRelay(1))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: MontyDisconnectError })
  t.regex(error.message, /^RuntimeError: monty worker connection closed while /)
})

test('a draining server rejects the checkout with MontyShutdown', async () => {
  const { url } = await server(drainingServer)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: MontyShutdown })
  t.is(error.message, 'RuntimeError: monty server is shutting down; the request did not run')
  t.is(error.dump, null)
})

test('a checkout that waits past checkoutTimeout rejects with a TimeoutError', async () => {
  const { url } = await server(relay)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30, maxProcesses: 1, checkoutTimeout: 0.1 })
  await using _held = await pool.checkout()
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: DOMException })
  t.is(error.name, 'TimeoutError')
  t.is(error.message, 'no monty worker became available within the checkout timeout')
})

test('a plain relay names no session, whatever ephemeral and profile say', async () => {
  const { url } = await server(relay)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  for (const options of [{}, { ephemeral: true }, { ephemeral: false }, { profile: 'gpu' }, { profile: '' }]) {
    await using session = await pool.checkout(options)
    t.is(session.sessionId, undefined)
    t.is(await session.feedRun('1 + 1'), 2)
  }
})

test('dump bytes from a plain relay load into a new session', async () => {
  const { url } = await server(relay)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  let dump: Buffer
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
  // request 1 is Configure, 2 the first feed
  const { url } = await server(droppingRelay(3))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
  const error = await t.throwsAsync(() => session.feedRun('2 + 2'), { instanceOf: MontyDisconnectError })
  t.regex(error.message, /^RuntimeError: monty worker connection closed while /)
  t.is(await t.throwsAsync(() => session.feedRun('3 + 3')), error)
})

test('a dropped connection during dump raises MontyDisconnectError', async () => {
  const { url } = await server(droppingRelay(2))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  await t.throwsAsync(() => session.dump(), { instanceOf: MontyDisconnectError })
})

test('a storing server names the session and a shutdown is resumed', async () => {
  const { url } = await server(new StoringServer().handle)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  const session = await pool.checkout()
  t.deepEqual(session.sessionId, Buffer.from('sess-1'))
  await session.feedRun('x = 20')
  // the fake parks the session instead of running this feed; the pool
  // reloads the ID on a new connection and re-sends it
  t.is(await session.feedRun('x + 1'), 21)
  await session.close()
  // the ID the resume adopted outlives the checkout
  t.deepEqual(session.sessionId, Buffer.from('sess-3'))
})

test('with autoResume off a shutdown raises MontyShutdown naming the session', async () => {
  const { url } = await server(new StoringServer().handle)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30, autoResume: false })
  {
    await using session = await pool.checkout()
    await session.feedRun('x = 20')
    const error = await t.throwsAsync(() => session.feedRun('x + 1'), { instanceOf: MontyShutdown })
    t.is(
      error.message,
      'RuntimeError: monty server is shutting down; the request did not run (restorable state attached)',
    )
    t.deepEqual(error.dump, Buffer.from('sess-1'))
  }
  await using session = await pool.checkout()
  await session.loadSession(Buffer.from('sess-1'))
  t.deepEqual(session.sessionId, Buffer.from('sess-3'))
  t.is(await session.feedRun('x + 1'), 21)
})

test('a closed pool rejects checkouts', async () => {
  const pool = await MontyRemote.create('http://127.0.0.1:9')
  await pool.close()
  await t.throwsAsync(() => pool.checkout(), {
    message: 'the pool is closed — create a new MontyRemote pool',
  })
})

test('MontyWebsocket is an alias of MontyRemote that still accepts ws:// URLs', async () => {
  t.is(MontyWebsocket, MontyRemote)
  const options: MontyWebsocketOptions = { requestTimeout: 30 }
  const remoteOptions: MontyRemoteOptions = options
  const { url } = await server(relay)
  await using pool = await MontyWebsocket.create(url.replace('http://', 'ws://'), remoteOptions)
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
})

test('a stream the server refuses before admitting it rejects the checkout with its status', async () => {
  const { url } = await server((stream) => endWithStatus(stream, status.RESOURCE_EXHAUSTED, 'at capacity', 'capacity'))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: MontyRefusedError })
  t.is(error.message, `RuntimeError: failed to spawn monty worker: ${url}: RESOURCE_EXHAUSTED: at capacity`)
  t.is(error.statusCode, 'RESOURCE_EXHAUSTED')
  t.is(error.statusMessage, 'at capacity')
  t.is(error.cause, 'capacity')
})

test('a status ending the stream is reported with its code, message and cause', async () => {
  const { url } = await server(async (stream) => {
    admit(stream)
    const requests = messages(stream)
    await requests.next() // Configure
    stream.write(OK_EVENT)
    await requests.next() // Feed
    endWithStatus(stream, status.DEADLINE_EXCEEDED, 'idle timeout', 'idle_timeout')
  })
  await using pool = await MontyRemote.create(url, { requestTimeout: 30, autoResume: false })
  await using session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyDisconnectError })
  t.is(error.message, 'RuntimeError: monty worker connection closed while waiting for a reply')
  t.is(error.statusCode, 'DEADLINE_EXCEEDED')
  t.is(error.statusMessage, 'idle timeout')
  t.is(error.cause, 'idle_timeout')
  t.is(error.transportError, null)
})

test('a stream the server simply ends reports an OK status', async () => {
  const { url } = await server(async (stream) => {
    admit(stream)
    const requests = messages(stream)
    await requests.next() // Configure
    stream.write(OK_EVENT)
    await requests.next() // Feed
    stream.end()
  })
  await using pool = await MontyRemote.create(url, { requestTimeout: 30, autoResume: false })
  await using session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyDisconnectError })
  t.is(error.statusCode, 'OK')
  t.is(error.statusMessage, null)
  t.is(error.cause, null)
})

test('a server ping is answered with a pong while the session is idle', async () => {
  let answered: Promise<boolean> | null = null
  const { url } = await server(async (stream) => {
    admit(stream)
    const requests = messages(stream)
    await requests.next() // Configure
    stream.write(OK_EVENT)
    // the client is now idle: nothing in flight, nothing awaited
    stream.write(PING_EVENT)
    answered = requests.next().then(({ value }) => value !== undefined && protoFields(value).has(12))
    await answered
    stream.end()
  })
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  const session = await pool.checkout()
  // hold the checkout idle while the server pings; its reply is a `Pong` request (tag 12)
  for (let i = 0; answered === null && i < 100; i++) await new Promise((resolve) => setTimeout(resolve, 10))
  t.is(await answered!, true)
  await session.close()
})
