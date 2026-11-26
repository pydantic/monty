// `MontyRemote` end-to-end against in-process gRPC servers that bridge each
// session stream to a real `monty subprocess` worker (see remote_servers.ts),
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
  executionErrorServer,
  messages,
  PING_EVENT,
  protoFields,
  refusingServer,
  relay,
  serve,
  StoringServer,
  type TestServer,
  vmLimitData,
} from './remote_servers.js'

import {
  type FailureClassification,
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

test('calls to async functions start without suspending the remote worker', async () => {
  const { url } = await server(relay)
  const log: string[] = []
  const fetch = async (x: number) => {
    log.push(`fetch(${x})`)
    const until = Date.now() + 300
    while (Date.now() < until) {
      // busy-wait: the sandbox only waits for this if the call suspended it
    }
    return x * 10
  }
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  const code =
    'import asyncio, time\nt0 = time.monotonic()\naws = [fetch(i) for i in range(3)]\n' +
    "elapsed = time.monotonic() - t0\nprint('started')\n[await asyncio.gather(*aws), elapsed < 0.25]"
  const result = await session.feedRun(code, {
    externalLookup: { fetch },
    printCallback: (_stream, text) => log.push(text),
  })
  // the sandbox did not wait for the host's 900ms of calls, so they did not suspend it
  t.deepEqual(result, [[0, 10, 20], true])
  // calls and prints reach the host in sandbox order, so each call started before the print after it
  t.deepEqual(log, ['fetch(0)', 'fetch(1)', 'fetch(2)', 'started\n'])
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

test('a session the server refuses fails its first run with MontyConfigurationError', async () => {
  const { url } = await server(refusingServer("unknown profile 'gpu'"))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout({ profile: 'gpu' })
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyConfigurationError })
  t.is(error.message, "session refused: unknown profile 'gpu'")
})

test('a connection dropped on the first request rejects it with MontyDisconnectError', async () => {
  const { url } = await server(droppingRelay(1))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyDisconnectError })
  t.regex(error.message, /^monty worker connection closed while /)
})

// ExcData field numbers.
const toolException = 4
const suspensionLimit = 5
test.each([
  ['ordinary marker', 'MemoryError', [toolException, []], { kind: 'ordinary' }],
  // limit = 0 and used = 2**53 + 1 distinguish missing and rounded values.
  [
    'memory limit',
    'MemoryError',
    vmLimitData(1, [0x08, 0, 0x10, 0x81, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x10]),
    { kind: 'memory', limit: 0n, used: 9_007_199_254_740_993n },
  ],
  ['feed time', 'TimeoutError', vmLimitData(2, [0x08, 30, 0x10, 31]), { kind: 'feed_time', limit: 30n, elapsed: 31n }],
  ['turn time', 'TimeoutError', vmLimitData(3, [0x08, 30, 0x10, 31]), { kind: 'turn_time', limit: 30n, elapsed: 31n }],
  [
    'recursion depth',
    'RecursionError',
    vmLimitData(4, [0x08, 5, 0x10, 6]),
    { kind: 'recursion_depth', limit: 5n, depth: 6n },
  ],
  ['suspension limit', 'RuntimeError', [suspensionLimit, [0x08, 0]], { kind: 'suspension_limit', limit: 0n }],
  ['incomplete suspension limit', 'RuntimeError', [suspensionLimit, []], null],
  ['unknown VM limit', 'RuntimeError', vmLimitData(), { kind: 'unknown' }],
  ['future VM limit arm', 'RuntimeError', vmLimitData(99), { kind: 'unknown' }],
  ['incomplete VM limit', 'MemoryError', vmLimitData(1, [0x08, 0]), { kind: 'unknown' }],
  ['future ExcData arm', 'MemoryError', [99, []], null],
  ['missing ExcData', 'MemoryError', null, null],
] satisfies [string, string, Parameters<typeof executionErrorServer>[1], FailureClassification | null][])(
  'a runtime error exposes %s independently of its message',
  async (_label, excType, wire, failure) => {
    const { url } = await server(executionErrorServer(excType, wire))
    await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
    await using session = await pool.checkout()
    const error = await t.throwsAsync(() => session.feedRun('1 + 1'), { instanceOf: MontyRuntimeError })
    t.deepEqual(error.exception, { typeName: excType, message: 'synthetic failure' })
    t.deepEqual(error.failure, failure)
    t.notThrows(() => JSON.stringify(error))
  },
)

test('explicit ordinary exceptions carry a marker without replacing structured exception data', async () => {
  const { url } = await server(relay)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  await session.feedRun('kept = 41')
  const ordinary = await t.throwsAsync(
    () => session.feedRun("raise MemoryError('memory limit exceeded: 8388609 bytes > 8388608 bytes')"),
    { instanceOf: MontyRuntimeError },
  )
  t.deepEqual(ordinary.failure, { kind: 'ordinary' })

  const valueError = await t.throwsAsync(() => session.feedRun("raise ValueError('bad input')"), {
    instanceOf: MontyRuntimeError,
  })
  t.deepEqual(valueError.failure, { kind: 'ordinary' })

  const jsonError = await t.throwsAsync(() => session.feedRun("import json; json.loads('!')"), {
    instanceOf: MontyRuntimeError,
  })
  t.is(jsonError.exception.typeName, 'json.JSONDecodeError')
  t.is(jsonError.failure, null)
  t.is(await session.feedRun('kept + 1'), 42)
})

test('only a VM recursion depth failure carries depth metadata, and the error remains catchable', async () => {
  const { url } = await server(relay)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout({ limits: { maxRecursionDepth: 5 } })
  await session.feedRun('def recurse():\n    return recurse()')

  const limitError = await t.throwsAsync(() => session.feedRun('recurse()'), { instanceOf: MontyRuntimeError })
  t.is(limitError.message, 'RecursionError: maximum recursion depth exceeded')

  const userError = await t.throwsAsync(
    () => session.feedRun("raise RecursionError('maximum recursion depth exceeded')"),
    { instanceOf: MontyRuntimeError },
  )
  t.deepEqual(userError.exception, limitError.exception)
  t.deepEqual(userError.failure, { kind: 'ordinary' })

  t.is(await session.feedRun('try:\n    recurse()\nexcept RecursionError:\n    caught = 42\ncaught'), 42)
  t.deepEqual(limitError.failure, { kind: 'recursion_depth', limit: 5n, depth: 6n })
})

test('the native pool reports the suspension limit and keeps the session usable', async () => {
  const { url } = await server(relay)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout({ limits: { maxSuspensions: 1 } })
  const externalLookup = { fetch: () => 'ok' }
  t.is(await session.feedRun("fetch('first')", { externalLookup }), 'ok')
  const error = await t.throwsAsync(() => session.feedRun("fetch('second')", { externalLookup }), {
    instanceOf: MontyRuntimeError,
  })
  t.deepEqual(error.exception, { typeName: 'RuntimeError', message: 'suspension limit 1 exceeded' })
  t.deepEqual(error.failure, { kind: 'suspension_limit', limit: 1n })
  t.is(await session.feedRun('1 + 1'), 2)
})

test('disconnecting while answering a host call does not report a result or replay the call', async () => {
  const { url, headers } = await server(droppingRelay(2))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  let calls = 0
  const feed = () =>
    session.feedRun('record() + 1', {
      externalLookup: { record: () => ++calls },
    })
  const error = await t.throwsAsync(feed, { instanceOf: MontyDisconnectError })
  t.is(error.statusCode, 'UNAVAILABLE')
  t.is(error.remoteCause, null)
  t.is(calls, 1)
  t.is(await t.throwsAsync(feed), error)
  t.is(calls, 1)
  t.is(headers.length, 1)
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
    t.is(await session.getSessionId(), undefined)
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
  t.is(await session.getSessionId(), undefined)
  t.is(await session.feedRun('x + 1'), 21)
})

test('a dropped connection raises MontyDisconnectError and loses the session', async () => {
  // request 1 is the Configure, carrying the first feed
  const { url } = await server(droppingRelay(2))
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
  // the deprecated getter is unset until the first run carries the Configure
  t.is(session.sessionId, undefined)
  await session.feedRun('x = 20')
  t.deepEqual(session.sessionId, Buffer.from('sess-1'))
  t.deepEqual(await session.getSessionId(), Buffer.from('sess-1'))
  // the fake parks the session instead of running this feed; the pool
  // reloads the ID on a new connection and re-sends it
  t.is(await session.feedRun('x + 1'), 21)
  await session.close()
  // the ID the resume adopted outlives the checkout
  t.deepEqual(await session.getSessionId(), Buffer.from('sess-2'))
})

test('getSessionId opens the session before any request', async () => {
  const { url } = await server(new StoringServer().handle)
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout()
  t.deepEqual(await session.getSessionId(), Buffer.from('sess-1'))
  t.is(await session.feedRun('x = 20'), null)
})

test('getSessionId raises a refused configuration and loses the session', async () => {
  const { url } = await server(refusingServer("unknown profile 'gpu'"))
  await using pool = await MontyRemote.create(url, { requestTimeout: 30 })
  await using session = await pool.checkout({ profile: 'gpu' })
  const error = await t.throwsAsync(() => session.getSessionId(), { instanceOf: MontyConfigurationError })
  t.is(error.message, "session refused: unknown profile 'gpu'")
  t.is(await t.throwsAsync(() => session.feedRun('1')), error)
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
  t.deepEqual(await session.getSessionId(), Buffer.from('sess-2'))
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
    // a session that is never fed sends nothing: drain until the half-close,
    // then hold the response side open
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
  t.deepEqual(extraRequests, [])
})

test('a status ending the stream is reported with its code, message and cause', async () => {
  const { url } = await server(async (stream) => {
    admit(stream)
    const requests = messages(stream)
    await requests.next() // Configure, carrying the first feed
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
    await requests.next() // Configure, carrying the first feed
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

/** Resolves once `condition` holds, polling for up to five seconds. */
async function waitFor(condition: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 500; attempt++) {
    if (condition()) return
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
  throw new Error('condition not met within 5s')
}

test('preWarm opens sessions before a checkout and replaces the one taken', async () => {
  const relayServer = await server(relay)
  await using pool = await MontyRemote.create(relayServer.url, { requestTimeout: 30, preWarm: 2, maxProcesses: 4 })
  t.is(relayServer.headers.length, 2)
  {
    await using session = await pool.checkout()
    t.is(await session.feedRun('1 + 1'), 2)
  }
  await waitFor(() => relayServer.headers.length === 3)
})

test('preWarm calls metadata for each ready session', async () => {
  const relayServer = await server(relay)
  let calls = 0
  await using pool = await MontyRemote.create(relayServer.url, {
    requestTimeout: 30,
    preWarm: 1,
    maxProcesses: 4,
    metadata: () => {
      calls += 1
      return { authorization: 'Bearer one' }
    },
  })
  {
    await using session = await pool.checkout()
    t.is(await session.feedRun('1 + 1'), 2)
  }
  await waitFor(() => relayServer.headers.length === 2)
  t.deepEqual(
    relayServer.headers.map((headers) => headers.authorization),
    ['Bearer one', 'Bearer one'],
  )
  // the two ready sessions, and the checkout's own call
  t.is(calls, 3)
})

test('a preWarm session opened with other metadata is not taken', async () => {
  const relayServer = await server(relay)
  let token = 'Bearer one'
  await using pool = await MontyRemote.create(relayServer.url, {
    requestTimeout: 30,
    preWarm: 1,
    maxProcesses: 4,
    metadata: () => ({ authorization: token }),
  })
  token = 'Bearer two'
  {
    await using session = await pool.checkout()
    t.is(await session.feedRun('1 + 1'), 2)
  }
  // the unused ready session, the checkout's own stream and the replacement
  await waitFor(() => relayServer.headers.length === 3)
  t.deepEqual(
    relayServer.headers.map((headers) => headers.authorization),
    ['Bearer one', 'Bearer two', 'Bearer two'],
  )
})

test('a metadata failure while pre-warming rejects create', async () => {
  const relayServer = await server(relay)
  const error = await t.throwsAsync(
    () =>
      MontyRemote.create(relayServer.url, {
        preWarm: 1,
        metadata: () => {
          throw new Error('no token')
        },
      }),
    { instanceOf: MontyConfigurationError },
  )
  t.is(error.message, 'metadata callback failed: no token')
})

test('preWarm above maxProcesses rejects create', async () => {
  const error = await t.throwsAsync(() => MontyRemote.create('http://127.0.0.1:1', { preWarm: 3, maxProcesses: 2 }), {
    instanceOf: MontyConfigurationError,
  })
  t.is(error.message, 'invalid pool size: pre_warm=3 max_processes=2')
})

test('a refused pre-warmed session rejects create', async () => {
  const { url } = await server((stream) => endWithStatus(stream, status.RESOURCE_EXHAUSTED, 'at capacity', 'capacity'))
  const error = await t.throwsAsync(() => MontyRemote.create(url, { preWarm: 1 }), { instanceOf: MontyRefusedError })
  t.is(error.message, `${url} refused the session: RESOURCE_EXHAUSTED (capacity): at capacity`)
})
