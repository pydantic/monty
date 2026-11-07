// `MontyRemote` end-to-end against in-process gRPC servers that bridge each
// session stream to a real `monty subprocess` child (see remote_servers.ts),
// mirroring crates/monty-python/tests/test_remote.py.

import { AsyncLocalStorage } from 'node:async_hooks'

import { Metadata, status } from '@grpc/grpc-js'
import { afterEach, test } from 'vitest'
import { t } from './assertions.js'
import {
  admit,
  CPU_CLOCK_OK_EVENT,
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
  MontyConfigurationError,
  MontyConnectError,
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
  result = { 'bad header': 't' }
  await t.throwsAsync(() => pool.checkout(), {
    instanceOf: MontyConfigurationError,
    message: `${relayServer.url}: metadata entry "bad header": invalid gRPC metadata key name`,
  })
  result = { 'x-token': 'a\nb' }
  await t.throwsAsync(() => pool.checkout(), {
    instanceOf: MontyConfigurationError,
    message: `${relayServer.url}: metadata entry "x-token" value: failed to parse metadata value`,
  })
  result = { 'x-token': 't' }
  await using session = await pool.checkout()
  t.is(await session.feedRun('1 + 1'), 2)
  t.is(relayServer.headers.length, 1)
})

test('a refused connection rejects the checkout', async () => {
  await using pool = await MontyRemote.create('http://127.0.0.1:9', { requestTimeout: 30 })
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: MontyConnectError })
  t.is(error.statusCode, null)
  t.is(error.statusMessage, null)
  t.is(error.message, `http://127.0.0.1:9: ${error.transportError}`)
})

test('a URL validation error carries its reason as the transport failure', async () => {
  const error = await t.throwsAsync(() => MontyRemote.create('http://localhost/sandbox'), {
    instanceOf: MontyConnectError,
  })
  const reason = 'a URL path, query or fragment is not supported; use only the scheme and host, with an optional port'
  t.is(error.statusCode, null)
  t.is(error.transportError, reason)
  t.is(error.message, `http://localhost/sandbox: ${reason}`)
})

// The session's configuration goes out with its first request, so a server
// refusing the session fails that request rather than the checkout.

test('a session the server refuses fails its first run with MontyRuntimeError', async () => {
  const { url } = await server(refusingServer("unknown profile 'gpu'"))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout({ profile: 'gpu' })
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyRuntimeError })
  t.is(error.message, "RuntimeError: unknown profile 'gpu'")
})

test('a connection dropped on the first request rejects it with MontyDisconnectError', async () => {
  const { url } = await server(droppingRelay(1))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyDisconnectError })
  t.regex(error.message, /^monty worker connection closed while /)
})

test('a draining server fails the first run with MontyShutdown', async () => {
  const { url } = await server(drainingServer)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyShutdown })
  t.is(error.message, 'monty server is shutting down; the request did not run')
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
  t.regex(error.message, /^monty worker connection closed while /)
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
  // the first run carries the Configure out, and its reply names the session
  t.is(session.sessionId, undefined)
  await session.feedRun('x = 20')
  t.deepEqual(session.sessionId, Buffer.from('sess-1'))
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
    t.is(error.message, 'monty server is shutting down; the request did not run (restorable state attached)')
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

test('every close() call waits for a finished session stream to end', async () => {
  let ended!: () => void
  const endSeen = new Promise<void>((resolve) => (ended = resolve))
  let release!: () => void
  const released = new Promise<void>((resolve) => (release = resolve))
  const { url } = await server(async (stream) => {
    admit(stream)
    const requests = messages(stream)
    await requests.next() // Configure
    stream.write(OK_EVENT)
    // drain until the half-close, then hold the response side open
    for await (const _ of requests);
    ended()
    await released
    stream.end()
  })
  const pool = await MontyRemote.create(url, { requestTimeout: 30 })
  const session = await pool.checkout()
  await session.close()
  await endSeen
  let settled = 0
  const closes = [pool.close(), pool.close()].map((close) => close.then(() => (settled += 1)))
  await new Promise((resolve) => setTimeout(resolve, 100))
  t.is(settled, 0)
  release()
  await Promise.all(closes)
  t.is(settled, 2)
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
  t.is(error.message, `${url} refused the session: RESOURCE_EXHAUSTED (capacity): at capacity`)
  t.is(error.statusCode, 'RESOURCE_EXHAUSTED')
  t.is(error.statusMessage, 'at capacity')
  t.is(error.remoteCause, 'capacity')
  t.is(Object.hasOwn(error, 'cause'), false)
  t.is(error.retryable, true)
  t.is(error.retryAfter, null)
})

test.each([
  [status.RESOURCE_EXHAUSTED, true],
  [status.UNAVAILABLE, true],
  [status.UNAUTHENTICATED, false],
  [status.PERMISSION_DENIED, false],
] as const)('retry metadata for refusal %s is preserved', async (code, retryable) => {
  const { url } = await server((stream) => {
    const metadata = new Metadata()
    metadata.set('retry-after', '12')
    stream.emit('error', { code, details: 'session unavailable', metadata })
  })
  await using pool = await MontyRemote.create(url)
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: MontyRefusedError })
  t.is(error.retryable, retryable)
  t.is(error.retryAfter, 12)
})

test('an unexpected server status is a typed connection error', async () => {
  const { url } = await server((stream) =>
    endWithStatus(stream, status.UNIMPLEMENTED, 'unknown service', 'protocol_violation'),
  )
  await using pool = await MontyRemote.create(url)
  const error = await t.throwsAsync(() => pool.checkout(), { instanceOf: MontyConnectError })
  t.is(error.statusCode, 'UNIMPLEMENTED')
  t.is(error.statusMessage, 'unknown service')
  t.is(error.remoteCause, 'protocol_violation')
  t.is(error.transportError, null)
  t.is(error.message, `${url}: UNIMPLEMENTED (protocol_violation): unknown service`)
})

test('a CPU-mode server requires a finite requestTimeout', async () => {
  const extraRequests: Buffer[] = []
  const { url } = await server(async (stream) => {
    admit(stream)
    const requests = messages(stream)
    await requests.next()
    stream.write(CPU_CLOCK_OK_EVENT)
    for await (const request of requests) extraRequests.push(request)
  })
  await using pool = await MontyRemote.create(url, { requestTimeout: null })
  const session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyConfigurationError })
  t.is(error.message, 'CPU execution clock requires a finite client request_timeout')
  // the feed was pipelined behind the Configure, but nothing follows it
  t.is(extraRequests.length, 1)
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
  t.is(
    error.message,
    'monty worker connection closed while waiting for a reply: DEADLINE_EXCEEDED (idle_timeout): idle timeout',
  )
  t.is(error.statusCode, 'DEADLINE_EXCEEDED')
  t.is(error.statusMessage, 'idle timeout')
  t.is(error.remoteCause, 'idle_timeout')
  t.is(Object.hasOwn(error, 'cause'), false)
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
  t.is(error.remoteCause, null)
})

test('a server ping is answered with a pong while the session is idle', async () => {
  let answered: Promise<boolean> | null = null
  const { url } = await server(async (stream) => {
    admit(stream)
    const requests = messages(stream)
    // the client is idle from the moment the stream opens: nothing goes out
    // until its first request
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
