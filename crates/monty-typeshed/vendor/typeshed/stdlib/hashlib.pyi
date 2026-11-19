# Based on python/typeshed's `stdlib/hashlib.pyi`, `_hashlib.pyi` and
# `_blake2.pyi`, cut down to what Monty implements at runtime — `file_digest`
# and `scrypt` are absent so type checking rejects them up front instead of
# letting them fail with `AttributeError`. Extend in lockstep with
# `crates/monty/src/modules/hashlib.rs`.
#
# Parameters typed `ReadableBuffer` upstream are `bytes` here: Monty has no
# `bytearray` or `memoryview`.

from typing import Final, Literal, final, overload

@final
class HASH:
    @property
    def digest_size(self) -> int: ...
    @property
    def block_size(self) -> int: ...
    @property
    def name(self) -> str: ...
    def copy(self) -> HASH: ...
    def digest(self) -> bytes: ...
    def hexdigest(self) -> str: ...
    def update(self, obj: bytes, /) -> None: ...

@final
class HASHXOF:
    @property
    def digest_size(self) -> int: ...
    @property
    def block_size(self) -> int: ...
    @property
    def name(self) -> str: ...
    def copy(self) -> HASHXOF: ...
    def digest(self, length: int) -> bytes: ...
    def hexdigest(self, length: int) -> str: ...
    def update(self, obj: bytes, /) -> None: ...

@final
class blake2b:
    MAX_DIGEST_SIZE: Final = 64
    MAX_KEY_SIZE: Final = 64
    PERSON_SIZE: Final = 16
    SALT_SIZE: Final = 16
    block_size: int
    digest_size: int
    name: str
    def __new__(
        cls,
        data: bytes = b'',
        /,
        *,
        digest_size: int = 64,
        key: bytes = b'',
        salt: bytes = b'',
        person: bytes = b'',
        fanout: int = 1,
        depth: int = 1,
        leaf_size: int = 0,
        node_offset: int = 0,
        node_depth: int = 0,
        inner_size: int = 0,
        last_node: bool = False,
        usedforsecurity: bool = True,
        string: bytes | None = None,
    ) -> blake2b: ...
    def copy(self) -> blake2b: ...
    def digest(self) -> bytes: ...
    def hexdigest(self) -> str: ...
    def update(self, data: bytes, /) -> None: ...

@final
class blake2s:
    MAX_DIGEST_SIZE: Final = 32
    MAX_KEY_SIZE: Final = 32
    PERSON_SIZE: Final = 8
    SALT_SIZE: Final = 8
    block_size: int
    digest_size: int
    name: str
    def __new__(
        cls,
        data: bytes = b'',
        /,
        *,
        digest_size: int = 32,
        key: bytes = b'',
        salt: bytes = b'',
        person: bytes = b'',
        fanout: int = 1,
        depth: int = 1,
        leaf_size: int = 0,
        node_offset: int = 0,
        node_depth: int = 0,
        inner_size: int = 0,
        last_node: bool = False,
        usedforsecurity: bool = True,
        string: bytes | None = None,
    ) -> blake2s: ...
    def copy(self) -> blake2s: ...
    def digest(self) -> bytes: ...
    def hexdigest(self) -> str: ...
    def update(self, data: bytes, /) -> None: ...

@overload
def new(
    name: Literal['shake_128', 'shake_256'],
    data: bytes = b'',
    *,
    usedforsecurity: bool = True,
    string: bytes | None = None,
) -> HASHXOF: ...
@overload
def new(
    name: Literal['blake2b'],
    data: bytes = b'',
    *,
    digest_size: int = 64,
    key: bytes = b'',
    salt: bytes = b'',
    person: bytes = b'',
    fanout: int = 1,
    depth: int = 1,
    leaf_size: int = 0,
    node_offset: int = 0,
    node_depth: int = 0,
    inner_size: int = 0,
    last_node: bool = False,
    usedforsecurity: bool = True,
    string: bytes | None = None,
) -> blake2b: ...
@overload
def new(
    name: Literal['blake2s'],
    data: bytes = b'',
    *,
    digest_size: int = 32,
    key: bytes = b'',
    salt: bytes = b'',
    person: bytes = b'',
    fanout: int = 1,
    depth: int = 1,
    leaf_size: int = 0,
    node_offset: int = 0,
    node_depth: int = 0,
    inner_size: int = 0,
    last_node: bool = False,
    usedforsecurity: bool = True,
    string: bytes | None = None,
) -> blake2s: ...
@overload
def new(name: str, data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def md5(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha1(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha224(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha256(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha384(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha512(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha3_224(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha3_256(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha3_384(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def sha3_512(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASH: ...
def shake_128(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASHXOF: ...
def shake_256(data: bytes = b'', *, usedforsecurity: bool = True, string: bytes | None = None) -> HASHXOF: ...
def pbkdf2_hmac(hash_name: str, password: bytes, salt: bytes, iterations: int, dklen: int | None = None) -> bytes: ...

algorithms_guaranteed: set[str]
algorithms_available: set[str]
