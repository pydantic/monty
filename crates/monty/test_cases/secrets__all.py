# /// monty
# call-external = true
# ///
import secrets

assert secrets.DEFAULT_ENTROPY == 32
old_entropy = secrets.DEFAULT_ENTROPY
secrets.DEFAULT_ENTROPY = 1
assert secrets.DEFAULT_ENTROPY == 1
assert len(secrets.token_bytes()) == 1
secrets.DEFAULT_ENTROPY = len
assert secrets.DEFAULT_ENTROPY([1]) == 1
secrets.DEFAULT_ENTROPY = old_entropy
assert isinstance(secrets.token_bytes(), bytes)
assert len(secrets.token_bytes(0)) == 0
assert len(secrets.token_bytes(17)) == 17
assert len(secrets.token_hex(17)) == 34
assert len(secrets.token_urlsafe(17)) <= 23
assert 0 <= secrets.randbits(24) < 2**24
assert secrets.randbits(0) == 0
assert secrets.randbits(k=0) == 0
assert 0 <= secrets.randbelow(7) < 7
assert 0 <= secrets.randbelow(2**100) < 2**100
assert secrets.randbelow(1) == 0
assert secrets.choice(['only']) == 'only'
assert secrets.choice(seq=['only']) == 'only'
assert secrets.choice('abc') in 'abc'

item = []
assert secrets.choice([item]) is item
secrets.choice([item]).append('mutated')
assert item == ['mutated']
cyclic = []
cyclic.append(cyclic)
assert secrets.choice([cyclic]) is cyclic
assert secrets.compare_digest(b'abc', b'abc')
assert not secrets.compare_digest(b'abc', b'abd')
assert secrets.compare_digest('ascii', 'ascii')
assert not secrets.compare_digest('abc', 'abd')

try:
    secrets.compare_digest(b'a', 'a')
except TypeError:
    pass
else:
    assert False, 'mixed types must fail'

try:
    secrets.compare_digest('é', 'é')
except TypeError:
    pass
else:
    assert False, 'non-ASCII strings must fail'
