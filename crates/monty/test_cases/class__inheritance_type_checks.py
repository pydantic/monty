# Instance and subclass checks follow the complete parent chain.

from collections import namedtuple


class Parent:
    pass


class Child(Parent):
    pass


class Grandchild(Child):
    pass


class Other:
    pass


instance = Grandchild()
assert type(instance) is Grandchild
assert instance.__class__ is Grandchild
assert isinstance(instance, Grandchild)
assert isinstance(instance, Child)
assert isinstance(instance, Parent)
assert isinstance(instance, (Other, Parent))
assert not isinstance(instance, Other)
assert not isinstance(Parent(), Child)
assert not isinstance(Child, Parent)

assert issubclass(Grandchild, Grandchild)
assert issubclass(Grandchild, Child)
assert issubclass(Grandchild, Parent)
assert issubclass(Grandchild, (Other, Parent))
assert not issubclass(Parent, Child)
assert not issubclass(Grandchild, Other)

assert issubclass(Grandchild, object)
assert issubclass(Grandchild, (Other, (Child,)))
assert issubclass(Grandchild, Other | Parent)
assert not issubclass(Parent, Child | Other)
assert not issubclass(Grandchild, ())
assert not issubclass(42, ())
assert issubclass(Grandchild, (Parent, 42))

assert issubclass(bool, int)
assert not issubclass(int, bool)
assert issubclass(int, object)
assert issubclass(type, type)
assert issubclass(type(None), object)
assert issubclass(ValueError, Exception)
assert issubclass(KeyError, LookupError)
assert not issubclass(BaseException, Exception)
assert issubclass(Exception, object)


assert not issubclass(Child, Exception)
assert not isinstance(Child(), Exception)

Point = namedtuple('Point', ['x'])
OtherPoint = namedtuple('Point', ['x'])
assert issubclass(Point, Point)
assert issubclass(Point, tuple)
assert issubclass(Point, object)
assert not issubclass(Point, OtherPoint)


def check_error(call, expected):
    try:
        call()
    except TypeError as error:
        assert str(error) == expected, str(error)
    else:
        assert False, 'expected TypeError'


for invalid in [42, Grandchild(), list[int], int | str]:
    check_error(lambda: issubclass(invalid, Parent), 'issubclass() arg 1 must be a class')

check_error(lambda: issubclass(42, 42), 'issubclass() arg 1 must be a class')
check_error(lambda: issubclass(Grandchild, 42), 'issubclass() arg 2 must be a class, a tuple of classes, or a union')
check_error(
    lambda: issubclass(Grandchild, (Other, 42)),
    'issubclass() arg 2 must be a class, a tuple of classes, or a union',
)
check_error(lambda: issubclass(int, list[int]), 'issubclass() argument 2 cannot be a parameterized generic')
check_error(lambda: issubclass(int, str | list[int]), 'issubclass() argument 2 cannot be a parameterized generic')
assert issubclass(int, int | list[int])
check_error(lambda: issubclass(), 'issubclass expected 2 arguments, got 0')
check_error(lambda: issubclass(int), 'issubclass expected 2 arguments, got 1')
check_error(lambda: issubclass(int, str, object), 'issubclass expected 2 arguments, got 3')
check_error(lambda: issubclass(int, classinfo=int), 'issubclass() takes no keyword arguments')
