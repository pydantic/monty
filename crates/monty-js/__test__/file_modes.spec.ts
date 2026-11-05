import { expect, test } from 'vitest'
import { MontyFileHandle, canonicalFileMode } from '../ts/types.js'

test.each(['w+b', 'wb+', '+bw'])('binary write/update mode %s is canonicalized', (mode) => {
  expect(canonicalFileMode(mode)).toBe('wb+')
  const handle = new MontyFileHandle('/file', mode)
  expect(handle.readable && handle.writable && handle.binary).toBe(true)
})

test.each(['r+', 'rb+', 'w+', 'a+b'])('other update modes remain rejected: %s', (mode) => {
  expect(() => canonicalFileMode(mode)).toThrow("update modes ('+') are not yet supported")
})
