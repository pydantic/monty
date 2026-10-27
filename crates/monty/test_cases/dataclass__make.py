# `dataclasses.make_dataclass`: a class built from a field list, then decorated.
import typing
from dataclasses import KW_ONLY, MISSING, InitVar, dataclass, field, fields, make_dataclass

# === Bare names, (name, type) and (name, type, Field) entries ===
C = make_dataclass('C', ['x', ('y', int), ('z', int, field(default=3))])
assert repr(C(1, 2)) == 'C(x=1, y=2, z=3)'
assert C.__name__ == 'C'
assert C.__annotations__ == {'x': typing.Any, 'y': int, 'z': int}
assert [f.name for f in fields(C)] == ['x', 'y', 'z']
assert fields(C)[1].type is int
assert C.__match_args__ == ('x', 'y', 'z')
assert C(1, 2) == C(1, 2)

# === A plain third item is the default ===
D = make_dataclass('D', [('a', int, 5)])
assert D().a == 5
assert D.a == 5

# === The options reach the decorator ===
F = make_dataclass('F', [('a', int)], frozen=True, order=True)
assert F(1) < F(2)
assert hash(F(1)) == hash((1,))
assert repr(F.__dataclass_params__) == (
    '_DataclassParams(init=True,repr=True,eq=True,order=True,unsafe_hash=False,'
    'frozen=True,match_args=True,kw_only=False,slots=False,weakref_slot=False)'
)
K = make_dataclass('K', ['a', 'b'], kw_only=True)
assert K(a=1, b=2).b == 2
assert K.__match_args__ == ()

# === namespace adds methods and attributes; the fields win a clash ===
N = make_dataclass('N', [('a', int, 5)], namespace={'double': lambda self: self.a * 2, 'a': 9, 'k': 1})
assert N().double() == 10
assert N.k == 1
assert N().a == 5
# Anything `dict.update` takes.
assert make_dataclass('Pairs', ['a'], namespace=[('m', 3)]).m == 3

# === module sets __module__ ===
assert make_dataclass('M', ['a'], module='mymod').__module__ == 'mymod'

# === The KW_ONLY, InitVar and ClassVar objects are recognised ===
W = make_dataclass('W', [('a', int), ('_', KW_ONLY), ('b', int)])
assert repr(W(1, b=2)) == 'W(a=1, b=2)'
assert W.__match_args__ == ('a',)
seen = []
V = make_dataclass(
    'V', [('a', int), ('iv', InitVar), ('b', int, 5)], namespace={'__post_init__': lambda self, iv: seen.append(iv)}
)
assert repr(V(1, 7)) == 'V(a=1, b=5)'
assert seen == [7]
G = make_dataclass('G', [('a', int), ('cv', typing.ClassVar)])
assert [f.name for f in fields(G)] == ['a']

# === A custom decorator receives the class and every option ===
calls = []


def deco(cls, **options):
    calls.append((cls.__name__, sorted(options), options['frozen']))
    return dataclass(cls, **options)


Y = make_dataclass('Y', ['a'], decorator=deco, frozen=1)
assert repr(Y(2)) == 'Y(a=2)'
assert calls == [
    ('Y', ['eq', 'frozen', 'init', 'kw_only', 'match_args', 'order', 'repr', 'slots', 'unsafe_hash', 'weakref_slot'], 1)
]

# === Field list errors ===
for fields_arg, message in [
    ([1], "object of type 'int' has no len()"),
    ([5.0], "object of type 'float' has no len()"),
    ([(1, int)], 'Field names must be valid identifiers: 1'),
    (['1a'], "Field names must be valid identifiers: '1a'"),
    (['if'], "Field names must not be keywords: 'if'"),
    (['a', 'a'], "Field name duplicated: 'a'"),
    ([('a',)], "Invalid field: ('a',)"),
    ([('a', int, 1, 2)], "Invalid field: ('a', <class 'int'>, 1, 2)"),
    (5, "'int' object is not iterable"),
]:
    try:
        make_dataclass('X', fields_arg)
        assert False, 'expected a TypeError'
    except TypeError as e:
        assert str(e) == message

# === Other argument errors ===
try:
    make_dataclass(5, ['a'])
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == 'type.__new__() argument 1 must be str, not int'
try:
    make_dataclass('X', ['a'], namespace=5)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not iterable"
try:
    make_dataclass('X')
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "make_dataclass() missing 1 required positional argument: 'fields'"
try:
    make_dataclass('X', ['a'], bogus=1)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "make_dataclass() got an unexpected keyword argument 'bogus'"
# `bases` must be a tuple: the empty tuple is accepted, anything else refused.
# `MISSING` is iterated like any other value, not mistaken for the omitted default.
assert make_dataclass('X', ['a'], bases=())(1).a == 1
try:
    iter(MISSING)
    assert False, 'expected a TypeError'
except TypeError as e:
    missing_not_iterable = str(e)
for bases, message in [
    ([], 'type.__new__() argument 2 must be tuple, not list'),
    (set(), 'type.__new__() argument 2 must be tuple, not set'),
    (MISSING, missing_not_iterable),
    (5, "'int' object is not iterable"),
]:
    try:
        make_dataclass('X', ['a'], bases=bases)
        assert False, 'expected a TypeError'
    except TypeError as e:
        assert str(e) == message
# The decorator's own checks still apply.
try:
    make_dataclass('X', [('a', int, [])])
    assert False, 'expected a ValueError'
except ValueError as e:
    assert str(e) == "mutable default <class 'list'> for field a is not allowed: use default_factory"
