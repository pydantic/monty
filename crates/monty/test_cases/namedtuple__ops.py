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

# === `in` / `not in` ===
# `vi` is sys.version_info, this file's namedtuple fixture.
assert vi.major in vi
assert vi.minor in vi
assert 9999 not in vi
# Membership uses tuple-style containment, allocating no iterator.
assert vi.micro in vi
assert 'not-a-field-value' not in vi
# The str field exercises the non-integer comparison path.
assert vi.releaselevel in vi
assert vi.serial in vi
# A probe that is a container is compared by value, not unpacked.
assert None not in vi
assert (vi.major, vi.minor) not in vi
assert [vi.major] not in vi
