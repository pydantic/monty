# `struct` module

Monty's `struct` is an illustrative subset built to demonstrate the native
module-registry seam, not a full implementation. It diverges from CPython in
several deliberate ways.

## Implemented

Functions: `calcsize`, `pack`, `unpack`.

Format codes: `b B h H i I l L q f d` (standard sizes), with a `<` / `>` / `!` /
`=` byte-order prefix and optional repeat counts (e.g. `4i`).

## Divergences from CPython

- **Format/value errors are `ValueError`, not `struct.error`.** Monty raises
  `ValueError` with its own messages where CPython raises `struct.error`; there
  is no `struct.error` attribute, and the message text differs. (A non-`bytes`
  buffer to `unpack` raises `TypeError`, which matches CPython.)
- **Most of the format language is absent.** No `Q`, `n`, `N`, `P`, `e`, `s`,
  `p`, `c`, `x`, and no `@` prefix — all raise `ValueError`. No native
  sizes/alignment: a no-prefix format is treated as standard-size (unlike
  CPython, where no prefix means native size/alignment). Large integers outside
  the `i64` range are rejected. Unsupported codes raise `ValueError`.
- **No `Struct` class, `iter_unpack`, `pack_into`, or `unpack_from`.**
- **Only `bytes` input buffers** for `unpack` (`bytearray`/`memoryview` do not
  exist in Monty).
