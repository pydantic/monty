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

# Extended introspection: type, namedtuple, builtin subtypes, and stdlib exceptions
from collections import Counter, defaultdict, namedtuple
from dataclasses import FrozenInstanceError
from datetime import date, datetime
import decimal

Point = namedtuple('Point', ['x', 'y'])
assert type.mro(type) == [type, object]
assert type.mro(A) == [A, object]

try:
    type.mro()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'unbound method type.mro() needs an argument'

try:
    getattr(type, 'mro')()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'unbound method type.mro() needs an argument'

m = type.mro
assert m(A) == [A, object]
assert m(int) == [int, object]

assert type.__name__ == 'type'
assert type.__mro__ == (type, object)

assert Point.__mro__ == (Point, tuple, object)
assert Point.mro() == [Point, tuple, object]
assert bool.__mro__ == (bool, int, object)
assert bool.mro() == [bool, int, object]
assert datetime.__bases__ == (date,)
assert datetime.__mro__ == (datetime, date, object)
assert datetime.mro() == [datetime, date, object]
assert defaultdict.__bases__ == (dict,)
assert defaultdict.__mro__ == (defaultdict, dict, object)
assert defaultdict.mro() == [defaultdict, dict, object]
assert Counter.mro() == [Counter, dict, object]

assert ValueError.__name__ == 'ValueError'
assert decimal.DivisionByZero.__name__ == 'DivisionByZero'
assert ValueError.__mro__ == (ValueError, Exception, BaseException, object)
assert ValueError.mro() == [ValueError, Exception, BaseException, object]
fn_ve_mro = ValueError.mro
assert fn_ve_mro() == [ValueError, Exception, BaseException, object]

assert decimal.DivisionByZero.__bases__ == (decimal.DecimalException, ZeroDivisionError)
assert decimal.DivisionUndefined.__bases__ == (decimal.InvalidOperation, ZeroDivisionError)
assert decimal.Overflow.__bases__ == (decimal.Inexact, decimal.Rounded)
assert decimal.Underflow.__bases__ == (decimal.Inexact, decimal.Rounded, decimal.Subnormal)
assert decimal.FloatOperation.__bases__ == (decimal.DecimalException, TypeError)
assert FrozenInstanceError.__bases__ == (AttributeError,)
assert UnboundLocalError.__bases__ == (NameError,)
assert ModuleNotFoundError.__bases__ == (ImportError,)
assert RecursionError.__bases__ == (RuntimeError,)
assert NotImplementedError.__bases__ == (RuntimeError,)
assert FileNotFoundError.__bases__ == (OSError,)

for bad_mro, expected_msg in [
    (lambda: type.mro(A, x=1), 'type.mro() takes no keyword arguments'),
    (
        lambda: type.mro(42),
        "descriptor 'mro' for 'type' objects doesn't apply to a 'int' object",
    ),
    (lambda: ValueError.mro(1), 'ValueError.mro() takes no arguments (1 given)'),
    (
        lambda: decimal.DivisionByZero.mro(1),
        'DivisionByZero.mro() takes no arguments (1 given)',
    ),
    (lambda: Point.mro(1), 'Point.mro() takes no arguments (1 given)'),
]:
    try:
        bad_mro()
        assert False, expected_msg
    except TypeError as e:
        assert str(e) == expected_msg

for bad_attr in [
    lambda: Point.missing([1]),
    lambda: ValueError.missing,
    lambda: ValueError.missing(1),
    lambda: type.missing,
    lambda: type.missing(1),
]:
    try:
        bad_attr()
        assert False, 'expected AttributeError'
    except AttributeError:
        pass

print('introspection tests passed')
