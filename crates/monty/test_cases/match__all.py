from collections import Counter, defaultdict, deque, namedtuple
from dataclasses import dataclass


# === Value patterns ===
def describe(x):
    match x:
        case 0:
            return 'zero'
        case 1 | 2 | 3:
            return 'small'
        case 'hello':
            return 'greeting'
        case 1.5:
            return 'float'
        case -1:
            return 'negative'
        case 1 + 2j:
            return 'complex'
        case b'raw':
            return 'bytes'
        case _:
            return 'other'


assert describe(0) == 'zero'
assert describe(0.0) == 'zero'
assert describe(False) == 'zero'
assert describe(2) == 'small'
assert describe('hello') == 'greeting'
assert describe(1.5) == 'float'
assert describe(-1) == 'negative'
assert describe(1 + 2j) == 'complex'
assert describe(b'raw') == 'bytes'
assert describe('zero') == 'other'
assert describe([0]) == 'other'


# === Singleton patterns use identity ===
def singleton(x):
    match x:
        case None:
            return 'none'
        case True:
            return 'true'
        case False:
            return 'false'
        case _:
            return 'other'


assert singleton(None) == 'none'
assert singleton(True) == 'true'
assert singleton(False) == 'false'
assert singleton(1) == 'other'
assert singleton(0) == 'other'
assert singleton(0.0) == 'other'

# === No case matching does nothing ===
result = 'untouched'
match 42:
    case 1:
        result = 'one'
    case 'x':
        result = 'x'
assert result == 'untouched'

# === Single irrefutable case ===
match 42:
    case value:
        pass
assert value == 42
match 42:
    case _:
        result = 'wildcard'
assert result == 'wildcard'


# === Dotted-name value patterns ===
class Color:
    RED = 1
    GREEN = 2


def color_name(c):
    match c:
        case Color.RED:
            return 'red'
        case Color.GREEN:
            return 'green'
    return 'unknown'


assert color_name(1) == 'red'
assert color_name(2) == 'green'
assert color_name(3) == 'unknown'


# === Guards ===
def sign(n):
    match n:
        case x if x < 0:
            return 'negative'
        case x if x == 0:
            return 'zero'
        case x:
            return 'positive'


assert sign(-5) == 'negative'
assert sign(0) == 'zero'
assert sign(7) == 'positive'

# a failing guard leaves the capture bound, like CPython
match 5:
    case guarded if guarded > 10:
        result = 'big'
    case _:
        result = 'small'
assert result == 'small'
assert guarded == 5

# === Capture and wildcard ===
match (1, 2):
    case (a, _):
        pass
assert a == 1


# === Sequence patterns ===
def seq(x):
    match x:
        case []:
            return 'empty'
        case [one]:
            return ('one', one)
        case [first, second]:
            return ('two', first, second)
        case [first, *rest]:
            return ('many', first, rest)


assert seq([]) == 'empty'
assert seq(()) == 'empty'
assert seq([7]) == ('one', 7)
assert seq((1, 2)) == ('two', 1, 2)
assert seq([1, 2, 3, 4]) == ('many', 1, [2, 3, 4])
assert seq(range(3)) == ('many', 0, [1, 2])
assert seq(deque([1, 2])) == ('two', 1, 2)
# str, bytes, sets, dicts and iterators are never sequences
assert seq('ab') is None
assert seq(b'ab') is None
assert seq({1, 2}) is None
assert seq({1: 2}) is None
assert seq(iter([1, 2])) is None
assert seq(42) is None

# star in the middle and at the start
match [1, 2, 3, 4, 5]:
    case [head, *middle, tail]:
        pass
assert (head, middle, tail) == (1, [2, 3, 4], 5)
match [1, 2, 3]:
    case [*init, last]:
        pass
assert (init, last) == ([1, 2], 3)
match [1]:
    case [*init, last]:
        pass
assert (init, last) == ([], 1)
match []:
    case [*everything]:
        pass
assert everything == []

# starred wildcard: items are indexed, nothing is collected
match [1, 2, 3, 4]:
    case [first, *_, last]:
        pass
assert (first, last) == (1, 4)
match [1, 2]:
    case [first, *_, last]:
        pass
assert (first, last) == (1, 2)
match [1]:
    case [first, *_, last]:
        result = 'matched'
    case _:
        result = 'too short'
assert result == 'too short'
match [10, 20, 30]:
    case [_, mid, *_]:
        pass
assert mid == 20
match [1, 2, 3]:
    case [*_]:
        result = 'any sequence'
assert result == 'any sequence'
match [1, 2, 3]:
    case [_, _]:
        result = 'two'
    case [_, _, _]:
        result = 'three'
assert result == 'three'

# nested sequences
match [[1, 2], [3, [4, 5]]]:
    case [[a, b], [c, [d, e]]]:
        pass
assert (a, b, c, d, e) == (1, 2, 3, 4, 5)

# sequence of values and captures
match (1, 'x', 3):
    case (1, label, 3):
        pass
assert label == 'x'

# a failed partial match leaves earlier bindings alone
a = 'before'
match [1, 2]:
    case [a, 3]:
        pass
    case _:
        pass
assert a == 'before'


# === Mapping patterns ===
def mapping(m):
    match m:
        case {'kind': 'point', 'x': x, 'y': y}:
            return ('point', x, y)
        case {'kind': 'empty', **rest}:
            return ('empty', rest)
        case {'kind': kind}:
            return ('kind', kind)
        case {}:
            return 'mapping'
        case _:
            return 'not a mapping'


assert mapping({'kind': 'point', 'x': 1, 'y': 2}) == ('point', 1, 2)
assert mapping({'kind': 'point', 'x': 1, 'y': 2, 'z': 3}) == ('point', 1, 2)
assert mapping({'kind': 'empty', 'extra': True}) == ('empty', {'extra': True})
assert mapping({'kind': 'empty'}) == ('empty', {})
assert mapping({'kind': 'other'}) == ('kind', 'other')
assert mapping({}) == 'mapping'
assert mapping({'x': 1}) == 'mapping'
assert mapping([('kind', 'point')]) == 'not a mapping'
assert mapping('kind') == 'not a mapping'

# dict subclasses are mappings; defaultdict lookups never create entries
dd = defaultdict(list)
match dd:
    case {'missing': _}:
        result = 'matched'
    case _:
        result = 'no match'
assert result == 'no match'
assert len(dd) == 0
match Counter('aab'):
    case {'a': count, **others}:
        pass
assert count == 2
assert others == {'b': 1}
assert type(others) is dict


# keys that compare equal at run time are a ValueError, checked in key order
class Alias:
    A = 1
    B = 1
    S = 'k'


try:
    match {1: 'x', 2: 'y'}:
        case {Alias.A: a, Alias.B: b}:
            pass
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'mapping pattern checks duplicate key (1)'
try:
    match {'k': 'x', 2: 'y'}:
        case {Alias.S: a, 'k': b}:
            pass
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == "mapping pattern checks duplicate key ('k')"
match {1: 'x', 2: 'y'}:
    case {Alias.A: a, 3: c, Alias.B: b}:
        result = 'matched'
    case _:
        result = 'missing key before the duplicate'
assert result == 'missing key before the duplicate'

# integers past 2**53 are distinct constant keys
match {9007199254740992: 'p', 9007199254740993: 'q'}:
    case {9007199254740992: p, 9007199254740993: q}:
        pass
assert (p, q) == ('p', 'q')

# non-string and dotted-name keys
match {1: 'one', Color.RED: 'red', None: 'nil'}:
    case {1: v1, None: v2}:
        pass
assert (v1, v2) == ('red', 'nil')
match {2: 'green'}:
    case {Color.GREEN: name}:
        pass
assert name == 'green'

# `**rest` alone and an empty remainder
match {'a': 1, 'b': 2}:
    case {**everything}:
        pass
assert everything == {'a': 1, 'b': 2}
match {'a': 1}:
    case {'a': 1, **nothing}:
        pass
assert nothing == {}

# nested mapping values
match {'user': {'name': 'ann', 'tags': ['x', 'y']}}:
    case {'user': {'name': str(who), 'tags': [*tags]}}:
        pass
assert (who, tags) == ('ann', ['x', 'y'])


# === Class patterns: builtins match themselves ===
def classify(x):
    match x:
        case bool(b):
            return ('bool', b)
        case int(n):
            return ('int', n)
        case float(f):
            return ('float', f)
        case str(s):
            return ('str', s)
        case bytes(b):
            return ('bytes', b)
        case list(items):
            return ('list', items)
        case tuple(items):
            return ('tuple', items)
        case dict(d):
            return ('dict', d)
        case set(s):
            return ('set', s)
        case frozenset(s):
            return ('frozenset', s)
        case _:
            return 'other'


assert classify(True) == ('bool', True)
assert classify(3) == ('int', 3)
assert classify(2.5) == ('float', 2.5)
assert classify('s') == ('str', 's')
assert classify(b's') == ('bytes', b's')
assert classify([1]) == ('list', [1])
assert classify((1,)) == ('tuple', (1,))
assert classify({'k': 1}) == ('dict', {'k': 1})
assert classify({1}) == ('set', {1})
assert classify(frozenset({1})) == ('frozenset', frozenset({1}))
assert classify(None) == 'other'
assert classify(1j) == 'other'


# bare class patterns and or-combinations
def text_like(x):
    match x:
        case str() | bytes():
            return True
        case _:
            return False


assert text_like('a')
assert text_like(b'a')
assert not text_like(1)

# keyword sub-patterns on builtins read attributes
match 3 + 4j:
    case complex(real=re, imag=im):
        pass
assert (re, im) == (3.0, 4.0)


# === Class patterns: dataclasses get __match_args__ ===
@dataclass
class Point:
    x: int
    y: int


assert Point.__match_args__ == ('x', 'y')


def where(p):
    match p:
        case Point(0, 0):
            return 'origin'
        case Point(x=0, y=y):
            return ('y-axis', y)
        case Point(x, 0):
            return ('x-axis', x)
        case Point(x, y) if x == y:
            return ('diagonal', x)
        case Point():
            return 'elsewhere'
        case _:
            return 'not a point'


assert where(Point(0, 0)) == 'origin'
assert where(Point(0, 5)) == ('y-axis', 5)
assert where(Point(3, 0)) == ('x-axis', 3)
assert where(Point(2, 2)) == ('diagonal', 2)
assert where(Point(1, 2)) == 'elsewhere'
assert where((0, 0)) == 'not a point'


@dataclass
class Line:
    start: Point
    end: Point


match Line(Point(1, 2), Point(3, 4)):
    case Line(Point(x1, y1), end=Point(x=x2, y=y2)):
        pass
assert (x1, y1, x2, y2) == (1, 2, 3, 4)


# === Class patterns: user classes with __match_args__ ===
class Pair:
    __match_args__ = ('left', 'right')

    def __init__(self, left, right):
        self.left = left
        self.right = right

    def total(self):
        return self.left + self.right


match Pair(1, 2):
    case Pair(l, r):
        pass
assert (l, r) == (1, 2)
match Pair(1, 2):
    case Pair(left=l, total=method):
        pass
assert l == 1
assert method() == 3


class Plain:
    def __init__(self):
        self.a = 1


def plain(obj):
    match obj:
        case Plain(a=1):
            return 'a is one'
        case Plain(b=_):
            return 'has b'
        case Plain():
            return 'plain'
    return 'other'


assert plain(Plain()) == 'a is one'
p = Plain()
p.a = 2
assert plain(p) == 'plain'
p.b = 3
assert plain(p) == 'has b'
assert plain(Pair(1, 2)) == 'other'


# a dataclass may define its own __match_args__
@dataclass
class Swapped:
    __match_args__ = ('b', 'a')
    a: int
    b: int


assert Swapped.__match_args__ == ('b', 'a')
match Swapped(1, 2):
    case Swapped(first, second):
        pass
assert (first, second) == (2, 1)

# === Class patterns: namedtuples ===
Coord = namedtuple('Coord', 'lat lon')
assert Coord.__match_args__ == ('lat', 'lon')
assert Coord(1, 2).__match_args__ == ('lat', 'lon')
match Coord(1.0, 2.0):
    case Coord(lat, lon):
        pass
assert (lat, lon) == (1.0, 2.0)
match Coord(1.0, 2.0):
    case Coord(lon=lon):
        pass
assert lon == 2.0
match Coord(1.0, 2.0):
    case [lat, lon]:
        result = 'namedtuple is a sequence'
assert result == 'namedtuple is a sequence'


# === Class patterns: exceptions ===
def exc_kind(exc):
    match exc:
        case KeyError(args=(key,)):
            return ('key', key)
        case LookupError():
            return 'lookup'
        case Exception(args=args):
            return ('exception', args)


assert exc_kind(KeyError('k')) == ('key', 'k')
assert exc_kind(IndexError('i')) == 'lookup'
assert exc_kind(ValueError('a')) == ('exception', ('a',))

# === Class pattern errors ===
try:
    match Plain():
        case Plain(x):
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Plain() accepts 0 positional sub-patterns (1 given)'
try:
    match Point(1, 2):
        case Point(1, 2, 3):
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Point() accepts 2 positional sub-patterns (3 given)'
try:
    match 1:
        case int(1, 2):
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'int() accepts 1 positional sub-pattern (2 given)'
try:
    match 1j:
        case complex(z):
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'complex() accepts 0 positional sub-patterns (1 given)'
try:
    match ValueError('x'):
        case ValueError(message):
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'ValueError() accepts 0 positional sub-patterns (1 given)'
try:
    match Point(1, 2):
        case Point(1, x=2):
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "Point() got multiple sub-patterns for attribute 'x'"
try:
    match 1:
        case len():
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'called match pattern must be a class'


# a repeated attribute is only reported once extraction reaches it
class OnlyA:
    __match_args__ = ('a',)


match OnlyA():
    case OnlyA(a, a=1):
        result = 'matched'
    case _:
        result = 'missing attribute wins'
assert result == 'missing attribute wins'


class BadArgs:
    __match_args__ = ['a']


class BadElements:
    __match_args__ = (1,)


try:
    match BadArgs():
        case BadArgs(q):
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'BadArgs.__match_args__ must be a tuple (got list)'
try:
    match BadElements():
        case BadElements(q):
            pass
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == '__match_args__ elements must be strings (got int)'
# elements past the ones used are not checked
match BadElements():
    case BadElements():
        result = 'no positional'
assert result == 'no positional'
# the isinstance check comes first, so a wrong type never reads __match_args__
match 1:
    case BadArgs(q):
        result = 'matched'
    case _:
        result = 'not an instance'
assert result == 'not an instance'


# === As patterns ===
def as_pattern(x):
    match x:
        case [1, 2] as pair:
            return ('pair', pair)
        case [int() as n, str() as s]:
            return ('int str', n, s)
        case (1 | 2 | 3) as small:
            return ('small', small)
        case _ as anything:
            return ('anything', anything)


assert as_pattern([1, 2]) == ('pair', [1, 2])
assert as_pattern((5, 'x')) == ('int str', 5, 'x')
assert as_pattern(2) == ('small', 2)
assert as_pattern('q') == ('anything', 'q')
match [[1, 2], 3]:
    case [[a, b] as inner, c]:
        pass
assert (inner, a, b, c) == ([1, 2], 1, 2, 3)


# === Or patterns ===
def or_pattern(x):
    match x:
        case 0 | None | '':
            return 'falsy-ish'
        case [a, b] | (a, b, _):
            return ('ab', a, b)
        case {'x': v} | {'y': v}:
            return ('v', v)
        case Point(x=v) | Pair(left=v):
            return ('class v', v)
    return 'nothing'


assert or_pattern(0) == 'falsy-ish'
assert or_pattern(None) == 'falsy-ish'
assert or_pattern('') == 'falsy-ish'
assert or_pattern([1, 2]) == ('ab', 1, 2)
assert or_pattern((1, 2, 3)) == ('ab', 1, 2)
assert or_pattern({'y': 9}) == ('v', 9)
assert or_pattern({'x': 8, 'y': 9}) == ('v', 8)
assert or_pattern(Point(4, 5)) == ('class v', 4)
assert or_pattern(Pair(6, 7)) == ('class v', 6)
assert or_pattern(1.5) == 'nothing'

# alternatives binding the same names in a different order
match [1, 2]:
    case [a, b] | [b, a, _]:
        pass
assert (a, b) == (1, 2)
match [1, 2, 3]:
    case [a, b] | [b, a, _]:
        pass
assert (a, b) == (2, 1)
match (1, 2, 0):
    case (0, a, b) | (a, b, 0):
        pass
assert (a, b) == (1, 2)
match (0, 'x', 'y'):
    case (0, a, b) | (a, b, 0):
        result = 'first alternative'
assert result == 'first alternative'
match ('p', 'q', 'r'):
    case [a, b, c] | [c, b, a, _] | [b, c, a, _, _]:
        pass
assert (a, b, c) == ('p', 'q', 'r')
match ('p', 'q', 'r', 's'):
    case [a, b, c] | [c, b, a, _] | [b, c, a, _, _]:
        pass
assert (a, b, c) == ('r', 'q', 'p')
match ('p', 'q', 'r', 's', 't'):
    case [a, b, c] | [c, b, a, _] | [b, c, a, _, _]:
        pass
assert (a, b, c) == ('r', 'p', 'q')

# or inside a sequence with captures
match [1, 'two']:
    case [1 | 2 as n, str(s) | bytes(s)]:
        pass
assert (n, s) == (1, 'two')


# === Nested match and control flow ===
def nested(x):
    match x:
        case [first, *rest]:
            match first:
                case int():
                    return ('int head', rest)
                case _:
                    return ('other head', rest)
        case _:
            return 'flat'


assert nested([1, 2]) == ('int head', [2])
assert nested(['a', 2]) == ('other head', [2])
assert nested(5) == 'flat'

collected = []
for item in [1, 'skip', 2, 'stop', 3]:
    match item:
        case 'skip':
            continue
        case 'stop':
            break
        case int(n):
            collected.append(n)
assert collected == [1, 2]


def early(x):
    match x:
        case 1:
            return 'one'
        case _:
            pass
    return 'fallthrough'


assert early(1) == 'one'
assert early(2) == 'fallthrough'

# match inside try/except with a raising body
try:
    match 'boom':
        case str(s):
            raise ValueError(s)
except ValueError as exc:
    assert str(exc) == 'boom'

# the subject expression is evaluated exactly once
calls = []


def subject():
    calls.append(1)
    return [1, 2]


match subject():
    case [1]:
        pass
    case [1, 2, 3]:
        pass
    case [1, x]:
        pass
assert x == 2
assert calls == [1]

# walrus in the subject and the guard
match (n := 10) + 1:
    case total if (doubled := total * 2) > 20:
        pass
assert (n, total, doubled) == (10, 11, 22)


# === Captures interact with scopes ===
def captures_are_locals():
    match [1, 2]:
        case [x, y]:
            pass
    return x + y


assert captures_are_locals() == 3


def closure_capture():
    match {'k': 'v'}:
        case {'k': captured}:
            pass
    return lambda: captured


assert closure_capture()() == 'v'

counter = 0


def bump():
    global counter
    match 5:
        case counter:
            pass


bump()
assert counter == 5


def outer():
    value = 'outer'

    def inner():
        nonlocal value
        match 'inner':
            case str(value):
                pass

    inner()
    return value


assert outer() == 'inner'


# === Value patterns call __eq__ ===
class Anything:
    def __eq__(self, other):
        return True


match Anything():
    case 'never literally equal':
        result = 'eq honoured'
assert result == 'eq honoured'
