import { expect, test } from 'vitest'

import { Monty } from '@pydantic/monty'

test('Monty.create runs feeds in a browser Web Worker', async () => {
  await using pool = await Monty.create()
  await using session = await pool.checkout()

  await session.feedRun('x = 2')
  const result = await session.feedRun('x + 3')

  expect({ result, crossOriginIsolated: globalThis.crossOriginIsolated }).toMatchObject({
    result: 5,
    crossOriginIsolated: false,
  })
})

test('browser wasm reports mounts as unsupported', async () => {
  await using pool = await Monty.create()
  await using session = await pool.checkout()

  await expect(session.feedRun("open('/mnt/data/file.txt').read()", { mount: [{}] as never })).rejects.toThrow(
    'the wasm worker does not support filesystem mounts',
  )
})
