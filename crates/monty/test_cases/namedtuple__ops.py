import sys

vi = sys.version_info

# === Equality: same object ===
assert vi == vi, 'namedtuple equals itself'

# === Equality: two references ===
vi2 = sys.version_info
assert vi == vi2, 'two refs to same namedtuple are equal'

# === Equality: namedtuple == equivalent tuple ===
t = (vi.major, vi.minor, vi.micro, vi.releaselevel, vi.serial)
assert vi == t, 'namedtuple equals equivalent tuple'
assert t == vi, 'equivalent tuple equals namedtuple'

# === Inequality: wrong length ===
assert vi != (3,), 'namedtuple not equal to wrong-length tuple'
assert (3,) != vi, 'wrong-length tuple not equal to namedtuple'

# === Inequality: different values ===
assert vi != (0, 0, 0, 'final', 0), 'namedtuple not equal to different values'

# === Inequality: non-tuple types ===
assert vi != 42, 'namedtuple not equal to int'
assert vi != 'hello', 'namedtuple not equal to str'
assert vi != None, 'namedtuple not equal to None'
assert vi != [3, 14], 'namedtuple not equal to list'

# === repr ===
r = repr(vi)
assert r.startswith('sys.version_info(major='), f'namedtuple repr starts with type name, {r!r}'
assert ', minor=' in r, f'namedtuple repr has minor field, {r!r}'
assert r.endswith(')'), f'namedtuple repr ends with paren, {r!r}'
