# `hashlib` module

Every algorithm in `hashlib.algorithms_guaranteed` is available, with no OpenSSL behind it (SHA-1 and SHA-2 use the
RustCrypto crates, the rest are implemented in Monty):
`md5`, `sha1`, `sha224`, `sha256`, `sha384`, `sha512`, `sha3_224`, `sha3_256`, `sha3_384`, `sha3_512`, `shake_128`,
`shake_256`, `blake2b` and `blake2s`, plus `new()`, `pbkdf2_hmac()`, `algorithms_guaranteed` and
`algorithms_available`.
Digests, the `name` / `digest_size` / `block_size` attributes, `update()` / `digest()` / `hexdigest()` / `copy()`,
the BLAKE2 parameters (`digest_size`, `key`, `salt`, `person` and the tree-hashing fields) and the error messages
match CPython 3.14 subject to the divergences below.
A hash object is ordinary session state: a dump taken between `update()` calls restores it mid-stream.

## Not implemented

- `file_digest()`.
    It reads from a file object in a loop, which a Monty builtin cannot do (see [open.md](open.md)).
- `scrypt()`.
- `hmac` is not importable (see [modules.md](modules.md)); `pbkdf2_hmac()` is the only HMAC construction.

## `algorithms_available` and `new()`

`algorithms_available` is the same set as `algorithms_guaranteed`.
In CPython it also lists what OpenSSL provides (`sha512_256`, `sha512_224`, `ripemd160`, `md5-sha1`, `sm3`, ...),
and `new()` accepts those names; Monty raises `ValueError: unsupported hash type sha512_256`.

`new()` accepts each algorithm's `hashlib` name exactly, plus OpenSSL's aliases for it case-insensitively
(`'SHA256'`, `'sha-256'`, `'sha3-256'`, `'shake128'`, `'blake2b512'`), as CPython does.
`new('blake2b512')` returns a `_blake2.blake2b` object; in CPython it is an OpenSSL-backed `_hashlib.HASH` whose
`name` is also `'blake2b'`.

`new()` with an unsupported name raises `ValueError` with CPython's message, but `pbkdf2_hmac()` raises the same
`ValueError: unsupported hash type <name>` where CPython raises its `ValueError` subclass
`_hashlib.UnsupportedDigestmodError` with the OpenSSL message `[digital envelope routines] unsupported`.
`pbkdf2_hmac()` with a SHAKE raises `ValueError: key length must be greater than 0.` when `dklen` is omitted and
`ValueError: [Provider routines] xof digests not allowed` otherwise; CPython's messages for these come from OpenSSL
and vary between builds.

## SHAKE digest lengths

`digest(length)` and `hexdigest(length)` on `shake_128` / `shake_256` objects raise `ValueError: negative digest length` for a negative length, where CPython 3.14 raises `SystemError` (`digest`) or `MemoryError` (`hexdigest`).
A length of `2**29` bytes or more raises `ValueError: digest length is too large`, the ceiling CPython's
`_sha3` module applies, where its OpenSSL-backed objects attempt the allocation and raise `MemoryError`.
Smaller lengths beyond the session's memory limit raise `MemoryError` (see
[resource_limits.md](resource_limits.md)).

## Windows integer widths

Monty's messages for out-of-range integers are the same on every platform; CPython's depend on the width of a
C `long`, which is 32 bits on Windows:

- BLAKE2 `leaf_size` above `2**32 - 1` raises `OverflowError: leaf_size is too large`; on Windows CPython the
    conversion overflows first with `Python int too large for C unsigned long`.
- `pbkdf2_hmac()` `iterations` or `dklen` above `2**31 - 1` raises `OverflowError: iteration value is too great.`
    or `key length is too great.`; on Windows CPython the conversion overflows first with
    `Python int too large to convert to C long`.

## Input types

Hash constructors, `update()`, the BLAKE2 `key` / `salt` / `person` parameters and `pbkdf2_hmac()`'s `password` /
`salt` take `bytes` only.
Monty has no `bytearray`, `memoryview` or `array`, so the "bytes-like object" the CPython docs describe is always
`bytes` here.
The `int` parameters (`digest_size` and the other BLAKE2 fields, a SHAKE `length`, `iterations` and `dklen`) take
`int` and `bool`; a class defining `__index__` raises `TypeError: 'X' object cannot be interpreted as an integer`
(see [classes.md](classes.md)).

## Method objects

`h.update` cannot be bound and called later: methods on hash objects can only be called in place
(`h.update(b'...')`), as for the other builtin types (see [classes.md](classes.md)).

## `copy` module

`copy.copy()` and `copy.deepcopy()` of a hash object raise `TypeError: cannot pickle '_hashlib.HASH' object`, as
in CPython; use `h.copy()`.
