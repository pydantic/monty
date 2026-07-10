import sys

vi = sys.version_info

# === Equality: same object ===
assert vi == vi

# === Equality: two references ===
vi2 = sys.version_info
assert vi == vi2

# === Equality: namedtuple == equivalent tuple ===
t = (vi.major, vi.minor, vi.micro, vi.releaselevel, vi.serial)
assert vi == t
assert t == vi

# === Inequality: wrong length ===
assert vi != (3,)
assert (3,) != vi

# === Inequality: different values ===
assert vi != (0, 0, 0, 'final', 0)

# === Inequality: non-tuple types ===
assert vi != 42
assert vi != 'hello'
assert vi != None
assert vi != [3, 14]

# === repr ===
r = repr(vi)
assert r.startswith('sys.version_info(major='), f'namedtuple repr starts with type name, {r!r}'
assert ', minor=' in r, f'namedtuple repr has minor field, {r!r}'
assert r.endswith(')'), f'namedtuple repr ends with paren, {r!r}'
