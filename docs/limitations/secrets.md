# `secrets` module

Monty's `secrets` provides token generation and random selection using entropy from the host's `os.urandom` call.
It does not use the seeded `random` generator.

## Implemented

- `choice(seq)`, `randbelow(exclusive_upper_bound)`, and `randbits(k)`
- `token_bytes(nbytes=None)`, `token_hex(nbytes=None)`, and `token_urlsafe(nbytes=None)`
- `compare_digest(a, b)` for bytes and ASCII strings

## Entropy

In a suspending runner, each entropy request pauses for the host.
Token sizes are checked against the session memory limit before generation.
Host replies must be bytes of exactly the requested length.
Rejection sampling may require multiple entropy requests, so hosts should answer each `os.urandom` call with the
requested number of bytes.
See [security.md](../security.md#entropy) for details on host entropy handling.

## Behavioural notes

- **No `SystemRandom`.** The public export is not implemented, so Monty's `secrets.__all__` omits it.
- **Integer limits.** `randbelow` accepts arbitrary-size positive integers, subject to the session's memory limits.
    `randbits` and token sizes are limited to a signed machine-sized integer.
- **`compare_digest`.** It accepts bytes and ASCII strings; non-ASCII strings and inputs of different types raise
    `TypeError`. Comparisons of equal-length inputs do not depend on their contents; as with CPython, input lengths are
    observable. Other buffer-protocol types such as `bytearray` are not supported.
- **`DEFAULT_ENTROPY`.** It is 32 bytes by default. Changing it affects token functions that omit `nbytes`, including
    directly imported functions. The value is shared across imports in the current VM session and persists across REPL
    snippets.
