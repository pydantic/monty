# Tests for class introspection: __bases__, __mro__, and mro()


class A:
    pass


class B(A):
    pass


class C(A, object):
    pass


class WithObject(object):
    pass


# Basic introspection on single inheritance
assert A.__bases__ == (object,)
assert A.__mro__ == (A, object)
assert A.mro() == [A, object]

assert B.__bases__ == (A,)
assert B.__mro__ == (B, A, object)
assert B.mro() == [B, A, object]

assert C.__bases__ == (A, object)
assert C.__mro__ == (C, A, object)
assert C.mro() == [C, A, object]

assert WithObject.__bases__ == (object,)
assert WithObject.__mro__ == (WithObject, object)
assert WithObject.mro() == [WithObject, object]


# Diamond inheritance introspection
class Root:
    pass


class Left(Root):
    pass


class Right(Root):
    pass


class Bottom(Left, Right):
    pass


assert Bottom.__bases__ == (Left, Right)
assert Bottom.__mro__ == (Bottom, Left, Right, Root, object)
assert Bottom.mro() == [Bottom, Left, Right, Root, object]


# Builtin types introspection
assert object.__bases__ == ()
assert object.__mro__ == (object,)
assert object.mro() == [object]

assert int.__bases__ == (object,)
assert int.__mro__ == (int, object)
assert int.mro() == [int, object]

assert list.__bases__ == (object,)
assert list.__mro__ == (list, object)
assert list.mro() == [list, object]

assert str.__bases__ == (object,)
assert str.__mro__ == (str, object)
assert str.mro() == [str, object]


# Error handling: mro() takes no arguments
try:
    A.mro(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'A.mro() takes no arguments (1 given)'

try:
    int.mro(1, 2)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int.mro() takes no arguments (2 given)'

print('introspection tests passed')
