# Tests for issubclass builtin function
from collections import Counter, defaultdict, namedtuple
from datetime import date, datetime


class Base:
    pass


class Derived(Base):
    pass


class Other:
    pass


class Root:
    pass


class Left(Root):
    pass


class Right(Root):
    pass


class Bottom(Left, Right):
    pass


# 1. User classes
assert issubclass(Base, Base)
assert issubclass(Derived, Base)
assert not issubclass(Base, Derived)
assert not issubclass(Derived, Other)
assert issubclass(Derived, object)
assert not issubclass(object, Derived)
assert not issubclass(object, Base)

assert issubclass(Bottom, Left)
assert issubclass(Bottom, Right)
assert issubclass(Bottom, Root)
assert issubclass(Bottom, object)
assert not issubclass(Left, Bottom)

# 2. Builtin types and subtypes
assert issubclass(int, int)
assert issubclass(bool, int)
assert not issubclass(int, bool)
assert issubclass(int, object)
assert not issubclass(object, int)
assert issubclass(object, object)
assert issubclass(datetime, date)
assert not issubclass(date, datetime)
assert issubclass(defaultdict, dict)
assert issubclass(Counter, dict)
assert not issubclass(dict, Counter)

# 3. Namedtuple
Point = namedtuple('Point', ['x', 'y'])
assert issubclass(Point, Point)
assert issubclass(Point, tuple)
assert issubclass(Point, object)
assert not issubclass(tuple, Point)
assert not issubclass(Point, int)

# 4. Exception types
assert issubclass(KeyError, LookupError)
assert issubclass(LookupError, Exception)
assert issubclass(Exception, BaseException)
assert issubclass(BaseException, object)
assert issubclass(KeyError, object)
assert not issubclass(KeyError, TypeError)
assert not issubclass(Exception, KeyError)

# 5. Tuples and Unions
assert issubclass(Derived, (Base, Other))
assert issubclass(Derived, (Other, Base))
assert not issubclass(Derived, (Other, int))
assert issubclass(Bottom, (int, (str, (Left,))))
assert not issubclass(Bottom, (int, (str, bytes)))
assert not issubclass(1, ())

# Unions
assert issubclass(Derived, Base | Other)
assert not issubclass(Derived, Other | int)

# Short-circuiting in tuple
assert issubclass(int, (int, 'not_a_type'))

# 6. Error handling
try:
    issubclass(1, int)
    assert False, 'expected TypeError for arg 1'
except TypeError as e:
    assert str(e) == 'issubclass() arg 1 must be a class'

try:
    issubclass(Base(), Base)
    assert False, 'expected TypeError for instance as arg 1'
except TypeError as e:
    assert str(e) == 'issubclass() arg 1 must be a class'

try:
    issubclass(Base, 1)
    assert False, 'expected TypeError for arg 2'
except TypeError as e:
    assert str(e) == 'issubclass() arg 2 must be a class, a tuple of classes, or a union'

try:
    issubclass(int, (str, 1))
    assert False, 'expected TypeError for invalid item in tuple arg 2'
except TypeError as e:
    assert str(e) == 'issubclass() arg 2 must be a class, a tuple of classes, or a union'

try:
    issubclass(int, list[int])
    assert False, 'expected TypeError for parameterized generic'
except TypeError as e:
    assert str(e) == 'issubclass() argument 2 cannot be a parameterized generic'

try:
    issubclass(list[int], list)
    assert False, 'expected TypeError for generic as arg 1'
except TypeError as e:
    assert str(e) == 'issubclass() arg 1 must be a class'

print('issubclass tests passed')
