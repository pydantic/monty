# Error paths of `hashlib`, with CPython 3.14's messages.
import hashlib

DATA_STRING = (
    "'data' and 'string' are mutually exclusive and support for 'string' keyword parameter "
    'is slated for removal in a future version.'
)


def raises(exc_type, func, *messages):
    # more than one message only where CPython's own wording depends on the platform's C `long` width
    try:
        func()
    except exc_type as exc:
        assert str(exc) in messages
    else:
        assert False, 'expected ' + exc_type.__name__


C_LONG_OVERFLOW = 'Python int too large to convert to C long'


# === constructor input ===
raises(TypeError, lambda: hashlib.sha256('abc'), 'Strings must be encoded before hashing')
raises(TypeError, lambda: hashlib.sha256(1), 'object supporting the buffer API required')
raises(TypeError, lambda: hashlib.sha256(None), 'object supporting the buffer API required')
raises(TypeError, lambda: hashlib.sha256(string=None), 'object supporting the buffer API required')
raises(TypeError, lambda: hashlib.sha256(string=1), 'object supporting the buffer API required')
raises(TypeError, lambda: hashlib.sha256(string='x'), 'Strings must be encoded before hashing')
raises(TypeError, lambda: hashlib.sha256(b'abc', string=b'd'), DATA_STRING)
raises(TypeError, lambda: hashlib.sha256(data=b'abc', string=b'd'), DATA_STRING)
raises(TypeError, lambda: hashlib.sha256(b'abc', string=None), DATA_STRING)
raises(TypeError, lambda: hashlib.sha256(None, string=b'a'), DATA_STRING)
raises(TypeError, lambda: hashlib.sha256(1, string=b'x'), DATA_STRING)
raises(TypeError, lambda: hashlib.sha256(b'abc', usedforsecurity=False, string=b'x'), DATA_STRING)
raises(TypeError, lambda: hashlib.blake2b(b'x', string=b'x'), DATA_STRING)
raises(TypeError, lambda: hashlib.blake2b(b'x', string=b'x', digest_size=0), DATA_STRING)
raises(TypeError, lambda: hashlib.blake2b('x'), 'Strings must be encoded before hashing')
raises(TypeError, lambda: hashlib.blake2b(1), 'object supporting the buffer API required')
raises(TypeError, lambda: hashlib.shake_128('x'), 'Strings must be encoded before hashing')

# === constructor signatures ===
raises(
    TypeError, lambda: hashlib.sha256(b'abc', b'd'), 'openssl_sha256() takes at most 1 positional argument (2 given)'
)
raises(
    TypeError,
    lambda: hashlib.sha256(b'a', b'b', foo=1),
    'openssl_sha256() takes at most 1 positional argument (2 given)',
)
raises(TypeError, lambda: hashlib.sha256(foo=1), "openssl_sha256() got an unexpected keyword argument 'foo'")
raises(TypeError, lambda: hashlib.sha256(b'a', foo=1), "openssl_sha256() got an unexpected keyword argument 'foo'")
raises(
    TypeError,
    lambda: hashlib.sha256(b'a', usedforsecurity=1, string=b'x', foo=1),
    'openssl_sha256() takes at most 3 arguments (4 given)',
)
raises(
    TypeError,
    lambda: hashlib.sha256(usedforsecurity=1, string=b'x', foo=1, bar=2),
    'openssl_sha256() takes at most 3 keyword arguments (4 given)',
)
raises(TypeError, lambda: hashlib.md5(b'a', b'b'), 'openssl_md5() takes at most 1 positional argument (2 given)')
raises(
    TypeError, lambda: hashlib.sha3_256(b'a', b'b'), 'openssl_sha3_256() takes at most 1 positional argument (2 given)'
)
raises(TypeError, lambda: hashlib.sha3_256(x=1), "openssl_sha3_256() got an unexpected keyword argument 'x'")
raises(
    TypeError,
    lambda: hashlib.shake_128(b'a', b'b'),
    'openssl_shake_128() takes at most 1 positional argument (2 given)',
)
raises(TypeError, lambda: hashlib.blake2b(b'a', b'b'), 'blake2b() takes at most 1 positional argument (2 given)')
raises(TypeError, lambda: hashlib.blake2b(b'a', b'b', b'c'), 'blake2b() takes at most 1 positional argument (3 given)')
raises(TypeError, lambda: hashlib.blake2b(b'a', b'b', foo=1), 'blake2b() takes at most 1 positional argument (2 given)')
raises(TypeError, lambda: hashlib.blake2b(foo=1), "blake2b() got an unexpected keyword argument 'foo'")
raises(TypeError, lambda: hashlib.blake2s(b'a', 1), 'blake2s() takes at most 1 positional argument (2 given)')

# === methods ===
raises(TypeError, lambda: hashlib.sha256().update('abc'), 'Strings must be encoded before hashing')
raises(TypeError, lambda: hashlib.sha256().update(1), 'object supporting the buffer API required')
raises(TypeError, lambda: hashlib.md5(b'a').update(), 'HASH.update() takes exactly one argument (0 given)')
raises(TypeError, lambda: hashlib.md5(b'a').update(b'a', b'b'), 'HASH.update() takes exactly one argument (2 given)')
raises(TypeError, lambda: hashlib.md5(b'a').update(data=b'b'), 'HASH.update() takes no keyword arguments')
raises(TypeError, lambda: hashlib.md5(b'a').update(b'b', c=1), 'HASH.update() takes no keyword arguments')
raises(TypeError, lambda: hashlib.sha256().digest(5), 'HASH.digest() takes no arguments (1 given)')
raises(TypeError, lambda: hashlib.sha3_256().digest(1), 'HASH.digest() takes no arguments (1 given)')
raises(TypeError, lambda: hashlib.md5(b'a').digest(x=1), 'HASH.digest() takes no keyword arguments')
raises(TypeError, lambda: hashlib.md5(b'a').hexdigest(1), 'HASH.hexdigest() takes no arguments (1 given)')
raises(TypeError, lambda: hashlib.md5(b'a').copy(1), 'HASH.copy() takes no arguments (1 given)')
raises(TypeError, lambda: hashlib.md5(b'a').copy(x=1), 'HASH.copy() takes no keyword arguments')
raises(TypeError, lambda: hashlib.shake_128().update(), 'HASH.update() takes exactly one argument (0 given)')
raises(TypeError, lambda: hashlib.blake2b().update(), 'blake2b.update() takes exactly one argument (0 given)')
raises(TypeError, lambda: hashlib.blake2b().update('x'), 'Strings must be encoded before hashing')
raises(TypeError, lambda: hashlib.blake2b(b'a').update(data=b'b'), 'blake2b.update() takes no keyword arguments')
raises(TypeError, lambda: hashlib.blake2b().digest(1), 'blake2b.digest() takes no arguments (1 given)')
raises(TypeError, lambda: hashlib.blake2b(b'a').digest(x=1), 'blake2b.digest() takes no keyword arguments')
raises(TypeError, lambda: hashlib.blake2b().hexdigest(1), 'blake2b.hexdigest() takes no arguments (1 given)')
raises(TypeError, lambda: hashlib.blake2s().copy(1), 'blake2s.copy() takes no arguments (1 given)')

# === shake digest lengths ===
raises(TypeError, lambda: hashlib.shake_128().digest(), "digest() missing required argument 'length' (pos 1)")
raises(TypeError, lambda: hashlib.shake_128().digest(foo=1), "digest() missing required argument 'length' (pos 1)")
raises(TypeError, lambda: hashlib.shake_128().hexdigest(), "hexdigest() missing required argument 'length' (pos 1)")
raises(TypeError, lambda: hashlib.shake_128().digest(3, 4), 'digest() takes at most 1 argument (2 given)')
raises(TypeError, lambda: hashlib.shake_128().digest(3, foo=1), 'digest() takes at most 1 argument (2 given)')
raises(
    TypeError,
    lambda: hashlib.shake_128().digest(length=3, foo=1),
    'digest() takes at most 1 keyword argument (2 given)',
)
raises(TypeError, lambda: hashlib.shake_128().digest('a'), "'str' object cannot be interpreted as an integer")
raises(TypeError, lambda: hashlib.shake_128().hexdigest(1.5), "'float' object cannot be interpreted as an integer")
raises(TypeError, lambda: hashlib.shake_128().digest(None), "'NoneType' object cannot be interpreted as an integer")

# === attributes ===
raises(AttributeError, lambda: hashlib.sha256().foo, "'_hashlib.HASH' object has no attribute 'foo'")
raises(AttributeError, lambda: hashlib.shake_128().foo, "'_hashlib.HASHXOF' object has no attribute 'foo'")
raises(AttributeError, lambda: hashlib.blake2b().foo, "'_blake2.blake2b' object has no attribute 'foo'")
raises(AttributeError, lambda: hashlib.sha256().foo(), "'_hashlib.HASH' object has no attribute 'foo'")
raises(AttributeError, lambda: hashlib.blake2s().foo(), "'_blake2.blake2s' object has no attribute 'foo'")
raises(
    AttributeError,
    lambda: setattr(hashlib.sha256(), 'name', 'x'),
    "attribute 'name' of '_hashlib.HASH' objects is not writable",
)
raises(
    AttributeError,
    lambda: setattr(hashlib.shake_128(), 'digest_size', 1),
    "attribute 'digest_size' of '_hashlib.HASHXOF' objects is not writable",
)
raises(
    AttributeError,
    lambda: setattr(hashlib.blake2b(), 'block_size', 1),
    "attribute 'block_size' of '_blake2.blake2b' objects is not writable",
)
raises(
    AttributeError,
    lambda: setattr(hashlib.sha256(), 'foo', 1),
    "'_hashlib.HASH' object has no attribute 'foo' and no __dict__ for setting new attributes",
)
raises(
    TypeError,
    lambda: hashlib.sha256() < hashlib.sha256(),
    "'<' not supported between instances of '_hashlib.HASH' and '_hashlib.HASH'",
)
raises(TypeError, lambda: len(hashlib.sha256()), "object of type '_hashlib.HASH' has no len()")
raises(TypeError, lambda: iter(hashlib.blake2b()), "'_blake2.blake2b' object is not iterable")
raises(TypeError, lambda: type(hashlib.sha256())(), "cannot create '_hashlib.HASH' instances")
raises(TypeError, lambda: type(hashlib.shake_256())(), "cannot create '_hashlib.HASHXOF' instances")

# === new() ===
raises(ValueError, lambda: hashlib.new('nope'), 'unsupported hash type nope')
raises(ValueError, lambda: hashlib.new(''), 'unsupported hash type ')
raises(ValueError, lambda: hashlib.new('Blake2b'), 'unsupported hash type Blake2b')
raises(ValueError, lambda: hashlib.new('SHAKE_128'), 'unsupported hash type SHAKE_128')
raises(ValueError, lambda: hashlib.new('Sha3_256'), 'unsupported hash type Sha3_256')
raises(TypeError, lambda: hashlib.new(1), "new() argument 'name' must be str, not int")
raises(TypeError, lambda: hashlib.new(b'sha256'), "new() argument 'name' must be str, not bytes")
raises(TypeError, lambda: hashlib.new(name=1), "new() argument 'name' must be str, not int")
raises(TypeError, lambda: hashlib.new(), "__hash_new() missing 1 required positional argument: 'name'")
raises(TypeError, lambda: hashlib.new(foo=1), "__hash_new() missing 1 required positional argument: 'name'")
raises(TypeError, lambda: hashlib.new('sha256', name='x'), "__hash_new() got multiple values for argument 'name'")
raises(TypeError, lambda: hashlib.new('sha256', 'abc'), 'Strings must be encoded before hashing')
raises(TypeError, lambda: hashlib.new('sha256', None), 'object supporting the buffer API required')
raises(TypeError, lambda: hashlib.new('nope', 1), 'object supporting the buffer API required')
raises(TypeError, lambda: hashlib.new('nope', 'x'), 'Strings must be encoded before hashing')
raises(TypeError, lambda: hashlib.new('sha256', b'a', b'b'), 'new() takes at most 2 positional arguments (3 given)')
raises(TypeError, lambda: hashlib.new('nope', b'a', b'b'), 'new() takes at most 2 positional arguments (3 given)')
raises(
    TypeError, lambda: hashlib.new('sha256', b'a', b'b', foo=1), 'new() takes at most 2 positional arguments (3 given)'
)
raises(TypeError, lambda: hashlib.new('sha256', b'a', foo=1), "new() got an unexpected keyword argument 'foo'")
raises(TypeError, lambda: hashlib.new('nope', foo=1), "new() got an unexpected keyword argument 'foo'")
raises(
    TypeError,
    lambda: hashlib.new('sha256', b'a', digest_size=16),
    "new() got an unexpected keyword argument 'digest_size'",
)
raises(TypeError, lambda: hashlib.new('sha256', data=b'a', string=b'b'), DATA_STRING)
raises(TypeError, lambda: hashlib.new('sha256', b'a', string=None), DATA_STRING)
raises(
    TypeError,
    lambda: hashlib.new('sha256', b'a', usedforsecurity=1, string=b'x', foo=1),
    'new() takes at most 4 arguments (5 given)',
)
raises(TypeError, lambda: hashlib.new('blake2b', foo=1), "blake2b() got an unexpected keyword argument 'foo'")
raises(TypeError, lambda: hashlib.new('blake2b', b'a', b'b'), 'blake2b() takes at most 1 positional argument (2 given)')
raises(TypeError, lambda: hashlib.new('blake2b', 'a'), 'Strings must be encoded before hashing')
raises(TypeError, lambda: hashlib.new('blake2b', data=b'a', string=b'a'), DATA_STRING)
raises(
    ValueError,
    lambda: hashlib.new('blake2s', digest_size=0),
    'digest_size for Blake2s must be between 1 and 32 bytes, here it is 0',
)

# === blake2 parameters, checked in signature order ===
raises(
    ValueError,
    lambda: hashlib.blake2b(digest_size=0),
    'digest_size for Blake2b must be between 1 and 64 bytes, here it is 0',
)
raises(
    ValueError,
    lambda: hashlib.blake2b(digest_size=65),
    'digest_size for Blake2b must be between 1 and 64 bytes, here it is 65',
)
raises(
    ValueError,
    lambda: hashlib.blake2b(digest_size=-1),
    'digest_size for Blake2b must be between 1 and 64 bytes, here it is -1',
)
raises(
    ValueError,
    lambda: hashlib.blake2s(digest_size=33),
    'digest_size for Blake2s must be between 1 and 32 bytes, here it is 33',
)
raises(OverflowError, lambda: hashlib.blake2b(digest_size=2**70), 'Python int too large to convert to C int')
raises(TypeError, lambda: hashlib.blake2b(digest_size='x'), "'str' object cannot be interpreted as an integer")
raises(TypeError, lambda: hashlib.blake2b(digest_size=1.5), "'float' object cannot be interpreted as an integer")
raises(TypeError, lambda: hashlib.blake2b(digest_size=None), "'NoneType' object cannot be interpreted as an integer")
raises(ValueError, lambda: hashlib.blake2b(key=b'x' * 65), 'maximum key length is 64 bytes')
raises(ValueError, lambda: hashlib.blake2s(key=b'x' * 33), 'maximum key length is 32 bytes')
raises(ValueError, lambda: hashlib.blake2b(salt=b'x' * 17), 'maximum salt length is 16 bytes')
raises(ValueError, lambda: hashlib.blake2s(salt=b'x' * 9), 'maximum salt length is 8 bytes')
raises(ValueError, lambda: hashlib.blake2b(person=b'x' * 17), 'maximum person length is 16 bytes')
raises(ValueError, lambda: hashlib.blake2s(person=b'x' * 9), 'maximum person length is 8 bytes')
raises(TypeError, lambda: hashlib.blake2b(key='x'), "a bytes-like object is required, not 'str'")
raises(TypeError, lambda: hashlib.blake2b(key=None), "a bytes-like object is required, not 'NoneType'")
raises(TypeError, lambda: hashlib.blake2b(salt='x'), "a bytes-like object is required, not 'str'")
raises(TypeError, lambda: hashlib.blake2b(person=1), "a bytes-like object is required, not 'int'")
raises(ValueError, lambda: hashlib.blake2b(fanout=256), 'fanout must be between 0 and 255')
raises(ValueError, lambda: hashlib.blake2b(fanout=-1), 'fanout must be between 0 and 255')
raises(OverflowError, lambda: hashlib.blake2b(fanout=2**70), 'Python int too large to convert to C int')
raises(TypeError, lambda: hashlib.blake2b(fanout='x'), "'str' object cannot be interpreted as an integer")
raises(ValueError, lambda: hashlib.blake2b(depth=0), 'depth must be between 1 and 255')
raises(ValueError, lambda: hashlib.blake2b(depth=256), 'depth must be between 1 and 255')
# `leaf_size` is a C `unsigned long`, so on Windows CPython the conversion itself overflows
raises(
    OverflowError,
    lambda: hashlib.blake2b(leaf_size=2**32),
    'leaf_size is too large',
    'Python int too large for C unsigned long',
)
raises(OverflowError, lambda: hashlib.blake2b(leaf_size=2**64), 'Python int too large for C unsigned long')
raises(ValueError, lambda: hashlib.blake2b(leaf_size=-1), 'Cannot convert negative int')
raises(TypeError, lambda: hashlib.blake2b(leaf_size='x'), "'str' object cannot be interpreted as an integer")
raises(OverflowError, lambda: hashlib.blake2b(node_offset=2**64), 'Python int too large for C unsigned long long')
raises(OverflowError, lambda: hashlib.blake2s(node_offset=2**48), 'node_offset is too large')
raises(ValueError, lambda: hashlib.blake2b(node_offset=-1), 'Cannot convert negative int')
raises(TypeError, lambda: hashlib.blake2b(node_offset='x'), "'str' object cannot be interpreted as an integer")
raises(ValueError, lambda: hashlib.blake2b(node_depth=256), 'node_depth must be between 0 and 255')
raises(ValueError, lambda: hashlib.blake2b(node_depth=-1), 'node_depth must be between 0 and 255')
raises(ValueError, lambda: hashlib.blake2b(inner_size=65), 'inner_size must be between 0 and is 64')
raises(ValueError, lambda: hashlib.blake2b(inner_size=-1), 'inner_size must be between 0 and is 64')
raises(ValueError, lambda: hashlib.blake2s(inner_size=33), 'inner_size must be between 0 and is 32')
# conversions run in signature order, before any range check
raises(TypeError, lambda: hashlib.blake2b(person=1, digest_size=0), "a bytes-like object is required, not 'int'")
raises(TypeError, lambda: hashlib.blake2b(key=1, fanout=-1), "a bytes-like object is required, not 'int'")
raises(TypeError, lambda: hashlib.blake2b(salt=1, leaf_size=2**32), "a bytes-like object is required, not 'int'")
raises(TypeError, lambda: hashlib.blake2b(key=1, digest_size='x'), "'str' object cannot be interpreted as an integer")
raises(
    OverflowError, lambda: hashlib.blake2b(fanout='x', digest_size=2**70), 'Python int too large to convert to C int'
)
raises(ValueError, lambda: hashlib.blake2b(depth=0, leaf_size=-1), 'Cannot convert negative int')
raises(ValueError, lambda: hashlib.blake2b(leaf_size=-1, node_depth='x'), 'Cannot convert negative int')
# then the range checks, in order, and the data last
raises(
    ValueError,
    lambda: hashlib.blake2b(digest_size=0, key=b'x' * 65),
    'digest_size for Blake2b must be between 1 and 64 bytes, here it is 0',
)
raises(
    ValueError,
    lambda: hashlib.blake2b(salt=b'x' * 17, person=b'x' * 17, key=b'x' * 65),
    'maximum salt length is 16 bytes',
)
raises(ValueError, lambda: hashlib.blake2b(person=b'x' * 17, fanout=0), 'maximum person length is 16 bytes')
raises(ValueError, lambda: hashlib.blake2b(fanout=-1, inner_size=65), 'fanout must be between 0 and 255')
raises(OverflowError, lambda: hashlib.blake2s(node_offset=2**48, inner_size=-1), 'node_offset is too large')
raises(ValueError, lambda: hashlib.blake2b(inner_size=-1, key=b'x' * 65), 'inner_size must be between 0 and is 64')
raises(ValueError, lambda: hashlib.blake2b(key=b'x' * 65, data='x'), 'maximum key length is 64 bytes')
raises(ValueError, lambda: hashlib.blake2b(data='x', fanout=-1), 'fanout must be between 0 and 255')
raises(
    ValueError,
    lambda: hashlib.blake2b(data=1, digest_size=0),
    'digest_size for Blake2b must be between 1 and 64 bytes, here it is 0',
)
raises(
    TypeError,
    lambda: hashlib.blake2b(
        b'a',
        **{
            'digest_size': 1,
            'key': b'',
            'salt': b'',
            'person': b'',
            'fanout': 1,
            'depth': 1,
            'leaf_size': 0,
            'node_offset': 0,
            'node_depth': 0,
            'inner_size': 0,
            'last_node': 0,
            'usedforsecurity': 1,
            'string': None,
            'foo': 1,
        },
    ),
    'blake2b() takes at most 14 arguments (15 given)',
)

# === pbkdf2_hmac ===
raises(ValueError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 0), 'iteration value must be greater than 0.')
raises(ValueError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', -1), 'iteration value must be greater than 0.')
# `iterations` and `dklen` are C `long`s, 32 bits on Windows, where the conversion itself overflows
raises(
    OverflowError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 2**31),
    'iteration value is too great.',
    C_LONG_OVERFLOW,
)
raises(
    OverflowError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 2**63),
    'Python int too large to convert to C long',
)
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1.5),
    "'float' object cannot be interpreted as an integer",
)
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 'x'),
    "'str' object cannot be interpreted as an integer",
)
raises(ValueError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, 0), 'key length must be greater than 0.')
raises(ValueError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, -1), 'key length must be greater than 0.')
# an explicit non-positive `dklen` is rejected before the digest is inspected, so a SHAKE gets the same message
raises(ValueError, lambda: hashlib.pbkdf2_hmac('shake_128', b'pw', b'salt', 1, 0), 'key length must be greater than 0.')
raises(
    OverflowError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, 2**31),
    'key length is too great.',
    C_LONG_OVERFLOW,
)
raises(
    OverflowError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, 2**64),
    'Python int too large to convert to C long',
)
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, 'x'),
    "'str' object cannot be interpreted as an integer",
)
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, 2.0),
    "'float' object cannot be interpreted as an integer",
)
raises(TypeError, lambda: hashlib.pbkdf2_hmac('sha256', 'pw', b'salt', 1), "a bytes-like object is required, not 'str'")
raises(TypeError, lambda: hashlib.pbkdf2_hmac('sha256', 1, b'salt', 1), "a bytes-like object is required, not 'int'")
raises(TypeError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw', 'salt', 1), "a bytes-like object is required, not 'str'")
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac(b'sha256', b'pw', b'salt', 1),
    "pbkdf2_hmac() argument 'hash_name' must be str, not bytes",
)
raises(TypeError, lambda: hashlib.pbkdf2_hmac(1, 1, 1, 1), "pbkdf2_hmac() argument 'hash_name' must be str, not int")
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt'),
    "pbkdf2_hmac() missing required argument 'iterations' (pos 4)",
)
raises(
    TypeError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw'), "pbkdf2_hmac() missing required argument 'salt' (pos 3)"
)
raises(TypeError, lambda: hashlib.pbkdf2_hmac(), "pbkdf2_hmac() missing required argument 'hash_name' (pos 1)")
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, 4, 5),
    'pbkdf2_hmac() takes at most 5 arguments (6 given)',
)
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, 4, foo=1),
    'pbkdf2_hmac() takes at most 5 arguments (6 given)',
)
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, foo=1),
    "pbkdf2_hmac() got an unexpected keyword argument 'foo'",
)
raises(
    TypeError,
    lambda: hashlib.pbkdf2_hmac(hash_name='sha256', password=b'pw', salt=b'salt', iterations=1, dklen=4, foo=1),
    'pbkdf2_hmac() takes at most 5 keyword arguments (6 given)',
)
# conversions in signature order, then the ordered checks
raises(TypeError, lambda: hashlib.pbkdf2_hmac('nope', 'pw', b'salt', 1), "a bytes-like object is required, not 'str'")
raises(TypeError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw', 1, 'x'), "a bytes-like object is required, not 'int'")
raises(
    ValueError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 0, 'x'), 'iteration value must be greater than 0.'
)
raises(
    OverflowError,
    lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 2**31, 'x'),
    'iteration value is too great.',
    C_LONG_OVERFLOW,
)
raises(
    ValueError, lambda: hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 0, 0), 'iteration value must be greater than 0.'
)
# CPython's wording for a SHAKE here comes from OpenSSL and differs between builds, so only the type is checked
for args in ((b'pw', b'salt', 2), (b'pw', b'salt', 3, 40)):
    try:
        hashlib.pbkdf2_hmac('shake_128', *args)
        assert False, 'expected ValueError'
    except ValueError:
        pass
