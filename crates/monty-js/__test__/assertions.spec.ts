import { expect, test } from 'vitest'
import { MontyRuntimeError } from '@pydantic/monty'
import { assertMemoryError } from './assertions.js'

test('memory assertions reject a different exception with a matching message', () => {
  const error = new MontyRuntimeError('ValueError', 'unrelated failure')
  error.message = 'MemoryError: memory limit exceeded: 120 bytes > 100 bytes'

  expect(() => assertMemoryError(error, 120, 100)).toThrow()
})
