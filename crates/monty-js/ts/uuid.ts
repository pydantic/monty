// Canonical uuid strings: the identities wrappers and host modules carry
// across the sandbox boundary.

/** Generates a canonical lowercase uuid4 string. `getRandomValues` rather than
 *  `randomUUID` so insecure browser contexts work too. */
export function generateUuid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 0x0f) | 0x40 // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80 // RFC 4122 variant
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

/** Canonical 8-4-4-4-12 hex uuid; case-insensitive since `normalizeId` lowercases. */
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** Validates a caller-supplied id and lowercases it, so the id the sandbox
 *  reports back (always lowercase) is the key the holder is found by. */
export function normalizeId(kind: string, id: string): string {
  if (typeof id !== 'string' || !UUID_PATTERN.test(id)) {
    throw new TypeError(`${kind} id must be a canonical uuid string, got ${JSON.stringify(id)}`)
  }
  return id.toLowerCase()
}
