# Tests for `collections.namedtuple`.
#
# Every expectation here is derived from CPython 3.14. This file is dual-run
# against CPython and Monty, so both must agree exactly.
#
# NOTE: construction arity/keyword errors are reported against the generated
# `__new__` (e.g. `Point.__new__() missing ...`), matching CPython 3.14 (see
# the `=== Construction errors ===` section).

from collections import namedtuple

Point = namedtuple('Point', ['x', 'y'])

# === Construction: positional, keyword, mixed ===
assert Point(11, 22).x == 11, 'positional construction, field x'
assert Point(x=1, y=2) == (1, 2), 'all-keyword construction'
assert Point(1, y=2) == (1, 2), 'mixed positional + keyword construction'

# === Field access: by name and by index ===
p = Point(11, 22)
assert p.x == 11, 'attribute access x'
assert p.y == 22, 'attribute access y'
assert p[0] == 11, 'index access [0]'
assert p[1] == 22, 'index access [1]'
assert p[-1] == 22, 'negative index access [-1]'

# === Type identity and name ===
# A factory-made namedtuple carries the user-supplied name (bare, not
# module-qualified), and all instances share one class object.
assert type(p).__name__ == 'Point', 'type name is the bare user name'
assert type(p) is Point, 'instance type is the class object the factory returned'
assert type(Point(1, 2)) is type(Point(3, 4)), 'all instances share one class'

# === __doc__ / __module__ ===
# CPython synthesises the docstring from `repr(field_names)` with the quotes
# stripped and the brackets removed, so a one-field class keeps the tuple comma.
assert Point.__doc__ == 'Point(x, y)', 'class docstring is synthesised from the fields'
assert namedtuple('S', 'a b c').__doc__ == 'S(a, b, c)', 'three fields'
assert namedtuple('Q', ['a']).__doc__ == 'Q(a,)', 'one field keeps the tuple comma'
assert namedtuple('E', []).__doc__ == 'E()', 'no fields'
assert namedtuple('R', ['def', 'x'], rename=True).__doc__ == 'R(_0, x)', 'docstring uses renamed fields'
assert namedtuple('D', 'x y', defaults=[1, 2]).__doc__ == 'D(x, y)', 'defaults do not appear'
assert p.__doc__ == 'Point(x, y)', 'instances inherit the class docstring'

# `module=` sets `__module__`; omitted or None falls back to the caller's __name__
assert Point.__module__ == '__main__', 'module defaults to the calling module'
assert namedtuple('M', 'a', module='mymod').__module__ == 'mymod', 'module= is honoured'
assert namedtuple('M', 'a', module=None).__module__ == '__main__', 'module=None falls back'
assert namedtuple('M', 'a', module=42).__module__ == 42, 'module= is stored unvalidated'
assert p.__module__ == '__main__', 'instances inherit the class module'

# === repr ===
assert repr(p) == 'Point(x=11, y=22)', 'repr uses field=value with the bare name'
assert repr(Point('a', 'b')) == "Point(x='a', y='b')", 'repr uses element repr'

# === Tuple semantics ===
assert p == (11, 22), 'namedtuple equals equivalent plain tuple'
assert (11, 22) == p, 'plain tuple equals equivalent namedtuple'
assert Point(1, 2) == Point(1, 2), 'namedtuples with equal fields are equal'
assert Point(1, 2) != Point(1, 3), 'namedtuples with differing fields are unequal'
assert len(p) == 2, 'len is the field count'
assert list(p) == [11, 22], 'iteration yields values in field order'
assert tuple(p) == (11, 22), 'tuple() round-trips the values'
Empty = namedtuple('Empty', [])
assert bool(Empty()) is False, 'empty namedtuple is falsey'
assert bool(p) is True, 'non-empty namedtuple is truthy'

# === Hashing ===
assert hash(p) == hash((11, 22)), 'hash matches the equivalent plain tuple'
assert {p: 'v'}[(11, 22)] == 'v', 'usable as a dict key, equal to the tuple key'
assert (11, 22) in {Point(11, 22)}, 'usable as a set member, equal to the tuple'

# === Field-spec forms: string (space / comma separated) and iterables ===
assert namedtuple('T', 'a b c')._fields == ('a', 'b', 'c'), 'space-separated field string'
assert namedtuple('T', 'a,b,c')._fields == ('a', 'b', 'c'), 'comma-separated field string'
assert namedtuple('T', 'a, b, c')._fields == ('a', 'b', 'c'), 'comma-and-space field string'
assert namedtuple('T', ['a', 'b'])._fields == ('a', 'b'), 'list field spec'
assert namedtuple('T', ('a', 'b'))._fields == ('a', 'b'), 'tuple field spec'

# === _fields / _field_defaults ===
assert Point._fields == ('x', 'y'), '_fields is a tuple of field names'
assert Point._field_defaults == {}, '_field_defaults empty when no defaults'
# both are class attributes an instance inherits, so they read the same either way
assert p._fields == ('x', 'y')
assert p._field_defaults == {}

# === _asdict / _replace / _make ===
assert p._asdict() == {'x': 11, 'y': 22}, '_asdict returns a plain dict'
assert p._replace(y=99) == (11, 99), '_replace overrides one field'
assert p._replace(y=99).x == 11, '_replace keeps the other field'
assert p == (11, 22), '_replace does not mutate the original'
assert Point._make([7, 8]) == (7, 8), '_make builds from an iterable'
assert type(Point._make([7, 8])) is Point, '_make returns the same class'

# === defaults (rightmost fields) ===
D = namedtuple('D', ['a', 'b', 'c'], defaults=[5, 6])
assert D._field_defaults == {'b': 5, 'c': 6}, 'defaults map to the rightmost fields'
# a populated `_field_defaults` reads identically off an instance
assert D(1)._field_defaults == {'b': 5, 'c': 6}
assert D(1) == (1, 5, 6), 'defaults fill omitted trailing fields'
assert D(1, 2) == (1, 2, 6), 'one supplied, one defaulted'
assert D(1, 2, 3) == (1, 2, 3), 'all supplied overrides defaults'

# === Heap-reference default values (exercises the class object's refcounting) ===
ListDefault = namedtuple('ListDefault', ['a', 'b'], defaults=[[9]])
assert ListDefault(0) == (0, [9]), 'heap-ref default applied to an omitted field'
assert ListDefault(0, 1) == (0, 1), 'provided value overrides the heap-ref default'
# Freeing a class whose default holds a heap reference must release it (regression
# for a missing NamedTupleClass arm in the GC walkers).
_tmp = namedtuple('Tmp', ['a'], defaults=[[1, 2]])
assert _tmp() == ([1, 2],), 'temporary class heap-ref default applied'
_tmp = 0

# === rename ===
R = namedtuple('R', ['abc', 'def', 'x', 'x'], rename=True)
assert R._fields == ('abc', '_1', 'x', '_3'), 'rename replaces keyword/dup fields by position'

# === Factory validation errors (ValueError, exact messages) ===
try:
    namedtuple('Point', ['x', 'x'])
    assert False, 'expected duplicate field name to raise'
except ValueError as e:
    assert str(e) == "Encountered duplicate field name: 'x'", 'duplicate field message'

try:
    namedtuple('Point', ['def'])
    assert False, 'expected keyword field name to raise'
except ValueError as e:
    assert str(e) == "Type names and field names cannot be a keyword: 'def'", 'keyword field message'

try:
    namedtuple('1bad', ['x'])
    assert False, 'expected invalid type name to raise'
except ValueError as e:
    assert str(e) == "Type names and field names must be valid identifiers: '1bad'", 'invalid type name message'

try:
    namedtuple('Point', ['x-y'])
    assert False, 'expected invalid field name to raise'
except ValueError as e:
    assert str(e) == "Type names and field names must be valid identifiers: 'x-y'", 'invalid field name message'

try:
    namedtuple('Point', ['_x'])
    assert False, 'expected underscore field name to raise'
except ValueError as e:
    assert str(e) == "Field names cannot start with an underscore: '_x'", 'underscore field message'

try:
    namedtuple('class', ['x'])
    assert False, 'expected keyword type name to raise'
except ValueError as e:
    assert str(e) == "Type names and field names cannot be a keyword: 'class'", 'keyword type name message'

# === Non-ASCII identifiers are accepted (full Unicode XID, like str.isidentifier) ===
# A field name that is XID-valid — including one built from a combining mark or an
# accented letter — must be accepted, not rejected as "not a valid identifier".
assert 'café'.isidentifier(), 'accented letters are valid identifiers'
assert 'á'.isidentifier(), 'a base letter plus a combining mark is a valid identifier'
assert namedtuple('T', ['café', 'á'])._fields == ('café', 'á'), 'Unicode field names are kept'
assert namedtuple('Café', ['x']).__name__ == 'Café', 'a non-ASCII type name is accepted'

# === Invalid-name error messages repr the name, quoting and escaping it ===
# CPython formats the offending name with `%r`, so a name containing a quote,
# backslash, or control character is escaped and may switch to double quotes.
try:
    namedtuple('P', ["a'b"])
    assert False, 'expected a quote in a field name to raise'
except ValueError as e:
    assert str(e) == 'Type names and field names must be valid identifiers: "a\'b"', (
        'single quote switches to double quotes'
    )

try:
    namedtuple('P', ['a\\b'])
    assert False, 'expected a backslash in a field name to raise'
except ValueError as e:
    assert str(e) == "Type names and field names must be valid identifiers: 'a\\\\b'", 'backslash is escaped'

try:
    namedtuple('P', ['a\nb'])
    assert False, 'expected a newline in a field name to raise'
except ValueError as e:
    assert str(e) == "Type names and field names must be valid identifiers: 'a\\nb'", 'control chars are escaped'

# === Instance errors ===
try:
    p.z
    assert False, 'expected missing attribute to raise'
except AttributeError as e:
    assert str(e) == "'Point' object has no attribute 'z'", 'missing attribute message'

try:
    p[5]
    assert False, 'expected out-of-range index to raise'
except IndexError as e:
    assert str(e) == 'tuple index out of range', 'index out of range message'

# === Construction errors ===
# CPython 3.14 reports these against the generated `__new__` (qualified by the
# type name); Monty matches that exactly.
try:
    Point(1)
    assert False, 'expected missing positional to raise'
except TypeError as e:
    assert str(e) == "Point.__new__() missing 1 required positional argument: 'y'", 'construction missing arg message'

try:
    Point(1, 2, 3)
    assert False, 'expected too many positionals to raise'
except TypeError as e:
    assert str(e) == 'Point.__new__() takes 3 positional arguments but 4 were given', (
        'construction too many args message'
    )

try:
    Point(1, 2, z=3)
    assert False, 'expected unexpected keyword to raise'
except TypeError as e:
    assert str(e) == "Point.__new__() got an unexpected keyword argument 'z'", 'construction unexpected keyword message'

try:
    Point(1, x=1)
    assert False, 'expected duplicate value to raise'
except TypeError as e:
    assert str(e) == "Point.__new__() got multiple values for argument 'x'", 'construction duplicate value message'

# === _make / _replace errors ===
try:
    Point._make([1])
    assert False, 'expected wrong-length _make to raise'
except TypeError as e:
    assert str(e) == 'Expected 2 arguments, got 1', '_make wrong-length message'

try:
    p._replace(z=9)
    assert False, 'expected unknown _replace field to raise'
except TypeError as e:
    assert str(e) == "Got unexpected field names: ['z']", '_replace unknown field message'

# === isinstance ===
# A namedtuple class is a real class object, so it works as `isinstance`'s
# second argument; and because a namedtuple IS a tuple subclass, `tuple` matches
# too.
Other = namedtuple('Other', ['z'])
q = Point(1, 2)
assert isinstance(q, Point), 'an instance matches its own class'
assert isinstance(q, tuple), 'a namedtuple is a tuple subclass'
assert not isinstance(q, Other), 'an instance does not match a different namedtuple class'
assert not isinstance((1, 2), Point), 'a plain tuple is not an instance of a namedtuple class'
assert not isinstance(q, dict), 'a namedtuple is not a dict'
# a tuple of classinfo entries, including nested tuples
assert isinstance(q, (int, Point)), 'matches inside a classinfo tuple'
assert not isinstance(q, (int, str)), 'no match inside a classinfo tuple'
assert isinstance(q, (int, (str, Point))), 'matches inside a nested classinfo tuple'

# === Inherited tuple surface ===
# A namedtuple is a tuple subclass, so membership, the tuple methods, and
# lexicographic ordering all work — including against a plain tuple, and across
# two different namedtuple classes (the class name takes no part).
Pair = namedtuple('Pair', ['x', 'y'])
Other2 = namedtuple('Other2', ['a', 'b'])
pt = Pair(11, 22)

# count() / index()
assert Pair(5, 5).count(5) == 2, 'count() tallies matching values'
assert Pair(5, 9).count(9) == 1, 'count() of a single match'
assert Pair(5, 9).count(0) == 0, 'count() of an absent value is 0'
assert pt.index(22) == 1, 'index() finds a value'
assert pt.index(11) == 0, 'index() of the first field'
try:
    pt.index(999)
    assert False, 'expected index() of a missing value to raise'
except ValueError as e:
    assert str(e) == 'tuple.index(x): x not in tuple', 'index() missing-value message'

# slicing, concatenation and repetition all degrade to a plain tuple, as in
# CPython — the field names describe only the original instance
tri = namedtuple('Tri', 'x y z')(1, 2, 3)
assert tri[1:] == (2, 3) and type(tri[1:]) is tuple, 'slice returns a plain tuple'
assert tri[:2] == (1, 2), 'slice from the start'
assert tri[::2] == (1, 3), 'strided slice'
assert tri[::-1] == (3, 2, 1), 'reversed slice'
assert tri[1:1] == (), 'empty slice'
assert tri[-2:] == (2, 3), 'negative slice bound'

assert tri + (4,) == (1, 2, 3, 4), 'namedtuple + tuple'
assert type(tri + (4,)) is tuple, 'concatenation returns a plain tuple'
assert (0,) + tri == (0, 1, 2, 3), 'tuple + namedtuple'
assert tri + tri == (1, 2, 3, 1, 2, 3), 'namedtuple + namedtuple'
assert pt + tri == (11, 22, 1, 2, 3), 'concatenating two different namedtuple classes'

assert tri * 2 == (1, 2, 3, 1, 2, 3), 'repetition'
assert type(tri * 2) is tuple, 'repetition returns a plain tuple'
assert 2 * tri == (1, 2, 3, 1, 2, 3), 'reflected repetition'
assert tri * 1 == (1, 2, 3), 'repetition by one'
assert tri * 0 == (), 'repetition by zero'
assert tri * -1 == (), 'negative repetition is empty'

# __getnewargs__ returns the plain tuple of values (used by copy/pickle)
assert tri.__getnewargs__() == (1, 2, 3), '__getnewargs__ returns the values'
assert type(tri.__getnewargs__()) is tuple, '__getnewargs__ returns a plain tuple'
assert namedtuple('One', 'a')(7).__getnewargs__() == (7,), '__getnewargs__ on a single field'

# membership
assert 11 in pt, 'membership hit on a field value'
assert 22 in pt, 'membership hit on the other field'
assert 99 not in pt, 'membership miss'

# lexicographic ordering
assert Pair(1, 2) < Pair(1, 3), '< compares elementwise'
assert Pair(1, 2) <= Pair(1, 2), '<= on equal values'
assert Pair(2, 0) > Pair(1, 9), '> is lexicographic (the first field decides)'
assert Pair(1, 2) >= Pair(1, 2), '>= on equal values'
assert not (Pair(1, 2) < Pair(1, 2)), '< is false for equal values'

# ordering against a plain tuple, in both directions
assert Pair(2, 0) > (1, 9), '> against a plain tuple'
assert Pair(1, 2) < (1, 3), '< against a plain tuple'
assert (1, 2) < Pair(1, 3), 'plain tuple on the left'
assert (1,) < Pair(1, 2), 'a shorter prefix sorts first'

# two different namedtuple classes compare by value
assert Pair(1, 2) < Other2(1, 3), 'ordering ignores the class name'

# and so sorting/min/max work
assert sorted([Pair(2, 1), Pair(1, 9)]) == [Pair(1, 9), Pair(2, 1)], 'sortable'
assert max(Pair(1, 2), Pair(1, 3)) == Pair(1, 3), 'max() over namedtuples'

# === rename= uses truthiness, not identity with True ===
assert namedtuple('T1', ['def'], rename=True)._fields == ('_0',), 'rename=True renames a keyword field'
assert namedtuple('T2', ['def'], rename=1)._fields == ('_0',), 'a truthy int enables renaming'
assert namedtuple('T3', ['def'], rename='yes')._fields == ('_0',), 'a truthy str enables renaming'
assert namedtuple('T4', ['x'], rename=0)._fields == ('x',), 'a falsy int leaves names alone'
assert namedtuple('T5', ['x'], rename=[])._fields == ('x',), 'an empty list is falsy'
assert namedtuple('T6', ['x'], rename=None)._fields == ('x',), 'None is falsy'


# === typename is coerced with str(), as in CPython ===
# CPython calls str(typename) before validating, so any object with a __str__
# that yields a valid identifier is accepted.
class _Named:
    def __str__(self):
        return 'Weird'


assert namedtuple(_Named(), ['a']).__name__ == 'Weird'
try:
    namedtuple(123, ['a'])
    assert False, 'expected a non-identifier typename to raise'
except ValueError as e:
    assert str(e) == "Type names and field names must be valid identifiers: '123'"

# === __qualname__ ===
# namedtuple sets __qualname__ = typename explicitly, so it always equals
# __name__ and never picks up a dotted path from an enclosing scope.
Qn = namedtuple('Qn', ['a'])
assert Qn.__qualname__ == 'Qn'
assert Qn.__qualname__ == Qn.__name__


def _make_nested():
    return namedtuple('Inner', ['a']).__qualname__


assert _make_nested() == 'Inner'

# === error messages name the class, not 'namedtuple' ===
Ord = namedtuple('Ord', 'x y')
try:
    Ord(1, 2) < [1, 3]
    assert False, 'expected ordering against a list to raise'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'Ord' and 'list'"

try:
    sorted([Ord(1, 2), [1, 3]])
    assert False, 'expected sorting a mixed list to raise'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'list' and 'Ord'"

# A string subscript reports as the tuple it subclasses rather than as
# 'namedtuple'. The exact wording still differs from CPython ("or slices",
# quoting) in a way plain tuples share, so it is pinned in
# crates/monty/tests/collections.rs rather than asserted here.
try:
    Ord(1, 2)['x']
    assert False, 'expected a string subscript to raise'
except TypeError:
    pass

# === Ordering edge cases ===
# A NaN element leaves the pair unordered rather than raising, in both
# directions. Distinct NaN objects deliberately — CPython short-circuits on
# identity before comparing, so a *shared* NaN behaves differently (see the
# identity-shortcut note in limitations/language.md).
_nan_a = float('nan')
_nan_b = float('nan')
assert not Ord(_nan_a, 1) < Ord(_nan_b, 2), 'a NaN element leaves the pair unordered'
assert not Ord(_nan_a, 1) > Ord(_nan_b, 2), 'unordered in the other direction too'

# Elements that are equal but individually unorderable do not block the
# comparison: equality is checked first, so the next element decides.
assert Ord(None, 1) < Ord(None, 2), 'equal unorderable elements are skipped'

# A genuinely differing unorderable pair still raises.
try:
    Ord(1, 'a') < Ord(1, 2)
    assert False, 'expected an unorderable element pair to raise'
except TypeError:
    pass
