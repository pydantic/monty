# `hashlib` module

Every algorithm in `hashlib.algorithms_guaranteed` is implemented, with no OpenSSL behind it, and a hash object is
ordinary session state: a dump taken between `update()` calls restores it mid-stream.

## Not implemented

- `file_digest()`.
    It reads from a file object in a loop, which a Monty builtin cannot do (see [open.md](open.md)).
- `scrypt()`.
- `hmac` is not importable (see [modules.md](modules.md)); `pbkdf2_hmac()` is the only HMAC construction.

## `algorithms_available` and `new()`

`algorithms_available` is the same set as `algorithms_guaranteed`.
In CPython it also lists what OpenSSL provides (`sha512_256`, `sha512_224`, `ripemd160`, `md5-sha1`, `sm3`, ...),
and `new()` accepts those names; Monty raises `ValueError: unsupported hash type sha512_256`.

`new('blake2b512')` returns a `_blake2.blake2b` object and `new('blake2s256')` a `_blake2.blake2s`; in CPython each
is an OpenSSL-backed `_hashlib.HASH` whose `name` is also `'blake2b'` / `'blake2s'`.

`pbkdf2_hmac()` with an unsupported name raises `ValueError: unsupported hash type <name>` where CPython raises its
`ValueError` subclass `_hashlib.UnsupportedDigestmodError` with the OpenSSL message
`[digital envelope routines] unsupported`.
With a SHAKE it raises `ValueError: key length must be greater than 0.` when `dklen` is omitted and
`ValueError: [Provider routines] xof digests not allowed` when it is positive; CPython's messages for these come from
OpenSSL and vary between builds.

## SHAKE digest lengths

`digest(length)` and `hexdigest(length)` on `shake_128` / `shake_256` objects reject a bad length with the messages
of CPython's `_sha3` module, where CPython's OpenSSL-backed objects behave differently:

- a negative length raises `ValueError: Cannot convert negative int`, where CPython raises `SystemError` (`digest`)
    or `MemoryError` (`hexdigest`);
- a length of `2**29` or more raises `ValueError: length is too large`, where CPython attempts the allocation and
    raises `MemoryError`;
- a length beyond `2**64 - 1` raises `OverflowError: Python int too large for C unsigned long`, where CPython says
    `Python int too large to convert to C ssize_t`.

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

`bytes` is the only bytes-like input: Monty has no `bytearray`, `memoryview` or `array`.
The `int` parameters take `int` and `bool`; a class defining `__index__` raises
`TypeError: 'X' object cannot be interpreted as an integer` (see [classes.md](classes.md)).

## Method objects

`h.update` cannot be bound and called later: methods on hash objects can only be called in place
(`h.update(b'...')`), as for the other builtin types (see [classes.md](classes.md)).
