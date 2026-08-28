import sys

vi = sys.version_info

# === Equality: same object ===
assert vi == vi
assert vi in [vi]
assert [vi].count(vi) == 1

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

# === The namedtuple API is not exposed on sys.version_info ===
# `sys.version_info` is a structseq, not a `collections.namedtuple`, so it has
# none of the `_`-prefixed helpers. Monty models both with one internal type,
# so these must stay gated on being built from a namedtuple *class*.
for _name in ('_fields', '_field_defaults', '_asdict', '_replace', '_make'):
    try:
        getattr(vi, _name)
        assert False, f'expected sys.version_info to have no {_name}'
    except AttributeError as e:
        assert str(e) == f"'sys.version_info' object has no attribute '{_name}'", f'{_name} message'

# calling them is likewise an AttributeError, not a working method
try:
    vi._asdict()
    assert False, 'expected _asdict() to raise'
except AttributeError as e:
    assert str(e) == "'sys.version_info' object has no attribute '_asdict'", '_asdict() message'

try:
    vi._replace(major=9)
    assert False, 'expected _replace() to raise'
except AttributeError as e:
    assert str(e) == "'sys.version_info' object has no attribute '_replace'", '_replace() message'

try:
    vi._make([1, 2, 3, 'f', 0])
    assert False, 'expected _make() to raise'
except AttributeError as e:
    assert str(e) == "'sys.version_info' object has no attribute '_make'", '_make() message'

# === Inherited tuple surface on a structseq ===
# `sys.version_info` models a CPython *structseq*, not a `collections.namedtuple`
# class, but slicing / concatenation / repetition are inherited from `tuple` all
# the same. Values are compared against `t` so the assertions stay
# version-independent.
assert vi[1:3] == t[1:3], 'structseq slice matches the equivalent tuple slice'
assert type(vi[1:3]) is tuple, 'slicing a structseq yields a plain tuple'
assert vi[:] == t, 'full slice reproduces the values'
assert vi[::-1] == t[::-1], 'reversed slice'
assert vi + (1,) == t + (1,), 'structseq + tuple'
assert type(vi + (1,)) is tuple, 'concatenation yields a plain tuple'
assert (1,) + vi == (1,) + t, 'tuple + structseq'
assert vi * 2 == t * 2, 'structseq repetition'
assert vi * 0 == (), 'repetition by zero'

# A structseq's `__new__` takes a single sequence, so its `__getnewargs__` wraps
# the values one level deeper than a namedtuple's (which returns them flat).
assert vi.__getnewargs__() == (t,), '__getnewargs__ wraps the values in a 1-tuple'
assert len(vi.__getnewargs__()) == 1, '__getnewargs__ has exactly one element'
