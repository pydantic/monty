import { expect, test } from 'vitest'

import { DispatchError, type Dispatcher } from '../ts/worker/host.js'
import { Writer, frame } from '../ts/worker/proto.js'
import { WorkerTransport } from '../ts/worker/transport.js'

const EMPTY = new Uint8Array()

/** Builds one framed ChildEvent with optional execution timing metadata. */
function childEvent(kind: number, payload = EMPTY, totalMicros?: number, maxMicros?: number): Uint8Array {
  const event = new Writer()
  event.lengthDelimited(kind, payload)
  if (totalMicros !== undefined) event.uint(20, totalMicros)
  if (maxMicros !== undefined) event.uint(21, maxMicros)
  return frame(event.finish())
}

/** Builds a timed turn-ending event that needs no value decoding. */
function typingErrorEvent(totalMicros: number, maxMicros: number): Uint8Array {
  const typingError = new Writer()
  typingError.string(1, 'diagnostic')
  return childEvent(8, typingError.finish(), totalMicros, maxMicros)
}

test('duration backstop ratchets from worker-reported execution time', async () => {
  const deadlines: (number | undefined)[] = []
  let call = 0
  const dispatcher: Dispatcher = async (_request, options) => {
    if (call++ === 0) return { reply: childEvent(10), status: 0 }
    deadlines.push(options?.timeoutMs)
    if (call === 2) return { reply: typingErrorEvent(50_000, 100_000), status: 0 }
    throw new DispatchError('deadline fired', true, 'exit code: 1')
  }
  const transport = await WorkerTransport.create(
    dispatcher,
    { limits: { maxDurationSecs: 0.1 } },
    { durationLimitGraceMs: 10, workerId: 1 },
  )

  expect((await transport.feed('pass', null, [], false, () => {})).kind).toBe('typingError')
  const crash = await transport.feed('pass', null, [], false, () => {})

  expect(deadlines).toEqual([110, 60])
  expect(crash).toEqual({ kind: 'crashed', message: 'deadline fired', timedOut: true, exitStatus: 'exit code: 1' })
})

test('restored sessions adopt the worker-reported duration budget', async () => {
  const deadlines: (number | undefined)[] = []
  let call = 0
  const dispatcher: Dispatcher = async (_request, options) => {
    switch (call++) {
      case 0:
        return { reply: childEvent(10), status: 0 }
      case 1:
        return { reply: childEvent(10, EMPTY, 25_000, 100_000), status: 0 }
      default:
        deadlines.push(options?.timeoutMs)
        throw new DispatchError('deadline fired', true)
    }
  }
  const transport = await WorkerTransport.create(dispatcher, {}, { durationLimitGraceMs: 10, workerId: 1 })

  await expect(transport.restore(new Uint8Array([1]), [], () => {})).resolves.toEqual({ kind: 'loaded' })
  await transport.feed('pass', null, [], false, () => {})

  expect(deadlines).toEqual([85])
})
