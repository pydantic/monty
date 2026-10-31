import secrets

for call in (lambda: secrets.token_bytes(-1), lambda: secrets.token_hex(-1), lambda: secrets.token_urlsafe(-1)):
    try:
        call()
    except ValueError as exc:
        assert str(exc) == 'negative argument not allowed'
    else:
        assert False, 'negative token size should fail'

for bits in (-1, -(2**100)):
    try:
        secrets.randbits(bits)
    except ValueError as exc:
        assert str(exc) == 'number of bits must be non-negative'
    else:
        assert False, 'negative bit count should fail'

for upper_bound in (0, False, -(2**100)):
    try:
        secrets.randbelow(upper_bound)
    except ValueError as exc:
        assert str(exc) == 'Upper bound must be positive.'
    else:
        assert False, 'non-positive upper bound should fail'

try:
    secrets.choice(seq=[])
except IndexError as exc:
    assert str(exc) == 'Cannot choose from an empty sequence'
else:
    assert False, 'empty choice should fail'

for keyword in ('sequence', 'Sequence'):
    try:
        secrets.choice(**{keyword: []})
    except TypeError as exc:
        assert str(exc) == f"Random.choice() got an unexpected keyword argument '{keyword}'"
    else:
        assert False, 'choice should reject an unknown keyword'

for helper, name in (
    (secrets.token_bytes, 'token_bytes'),
    (secrets.token_hex, 'token_hex'),
    (secrets.token_urlsafe, 'token_urlsafe'),
):
    try:
        helper(1, 2)
    except TypeError as exc:
        assert str(exc) == f'{name}() takes from 0 to 1 positional arguments but 2 were given'
    else:
        assert False, 'extra token argument should fail'

try:
    secrets.randbelow(n=3)
except TypeError as exc:
    assert str(exc) == "randbelow() got an unexpected keyword argument 'n'"
else:
    assert False, 'randbelow uses exclusive_upper_bound, not n'

try:
    secrets.randbelow(None)
except TypeError as exc:
    assert str(exc) == "'<=' not supported between instances of 'NoneType' and 'int'"
else:
    assert False, 'randbelow should compare its argument to zero'

try:
    secrets.randbits(-1.2)
except ValueError as exc:
    assert str(exc) == 'number of bits must be non-negative'
else:
    assert False, 'negative float bit count should fail'

for value, message in (
    ('x', "'<' not supported between instances of 'str' and 'int'"),
    (None, "'<' not supported between instances of 'NoneType' and 'int'"),
    (1.2, "'float' object cannot be interpreted as an integer"),
):
    try:
        secrets.randbits(value)
    except TypeError as exc:
        assert str(exc) == message
    else:
        assert False, 'randbits should reject invalid bit counts'

try:
    secrets.randbits(2**100)
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'
else:
    assert False, 'oversized randbits count should overflow'

try:
    secrets.randbelow(-1.2)
except ValueError as exc:
    assert str(exc) == 'Upper bound must be positive.'
else:
    assert False, 'randbelow should reject a negative float bound'

try:
    secrets.randbelow(1.2)
except AttributeError as exc:
    assert str(exc) == "'float' object has no attribute 'bit_length'"
else:
    assert False, 'randbelow should fail when a positive float lacks bit_length'

try:
    secrets.compare_digest(a=b'x', b=b'x')
except TypeError as exc:
    assert str(exc) == '_hashlib.compare_digest() takes no keyword arguments'
else:
    assert False, 'compare_digest rejects keyword arguments'

try:
    secrets.compare_digest(1, 1)
except TypeError as exc:
    assert str(exc) == "unsupported operand types(s) or combination of types: 'int' and 'int'"
else:
    assert False, 'compare_digest only accepts bytes-like values or ASCII strings'

try:
    secrets.compare_digest(1, b'x')
except TypeError as exc:
    assert str(exc) == "a bytes-like object is required, not 'int'"
else:
    assert False, 'compare_digest requires bytes-like input when either operand is bytes'
