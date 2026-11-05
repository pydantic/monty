# Tests for object.__new__, Cls.__new__, and ExcType.__new__ safety and descriptor parity.


# === object.__new__ rejects built-in and user exception classes ===
for exc_cls in (BaseException, Exception, ValueError, KeyError):
    try:
        object.__new__(exc_cls)
        assert False, f'expected TypeError for {exc_cls}'
    except TypeError as err:
        assert str(err) == f'object.__new__({exc_cls.__name__}) is not safe, use {exc_cls.__name__}.__new__()'


class CustomErr(ValueError):
    pass


try:
    object.__new__(CustomErr)
    assert False, 'expected TypeError for CustomErr'
except TypeError as err:
    assert str(err) == 'object.__new__(CustomErr) is not safe, use CustomErr.__new__()'


# === object.__new__ excess-argument rules ===
class Bare:
    pass


try:
    object.__new__(Bare, 1)
    assert False, 'expected TypeError for Bare excess pos arg'
except TypeError as err:
    assert str(err) == 'Bare() takes no arguments'

try:
    object.__new__(Bare, k=1)
    assert False, 'expected TypeError for Bare excess kwarg'
except TypeError as err:
    assert str(err) == 'Bare() takes no arguments'


class OverridesInit:
    def __init__(self, x, y=2):
        self.x = x
        self.y = y


raw = object.__new__(OverridesInit, 10, y=20)
assert isinstance(raw, OverridesInit)
assert not hasattr(raw, 'x')


class OverridesNew:
    def __new__(cls, *args, **kwargs):
        return object.__new__(cls)


try:
    object.__new__(OverridesNew, 1)
    assert False, 'expected TypeError when __new__ is overridden'
except TypeError as err:
    assert str(err) == 'object.__new__() takes exactly one argument (the type to instantiate)'

try:
    object.__new__(OverridesNew, k=1)
    assert False, 'expected TypeError when __new__ is overridden with kwargs'
except TypeError as err:
    assert str(err) == 'object.__new__() takes exactly one argument (the type to instantiate)'


# === Descriptor / getattr parity for __new__, __init__, and mro ===
fn_bare_new = Bare.__new__
b_inst = fn_bare_new(Bare)
assert isinstance(b_inst, Bare)
assert Bare.__init__(b_inst) is None
assert b_inst.__init__() is None
fn_mro = Bare.mro
assert fn_mro() == [Bare, object]

fn_ve_new = ValueError.__new__
ce1 = fn_ve_new(CustomErr, 'msg', 42)
assert isinstance(ce1, CustomErr) and ce1.args == ('msg', 42)

fn_ce_new = CustomErr.__new__
ce2 = fn_ce_new(CustomErr, 'second')
assert isinstance(ce2, CustomErr) and ce2.args == ('second',)

fn_ce_init = CustomErr.__init__
fn_ce_init(ce2, 'reinit', 99)
assert ce2.args == ('reinit', 99)

for bad_call, expected_msg in [
    (lambda: ValueError.__new__(), 'ValueError.__new__(): not enough arguments'),
    (lambda: ValueError.__new__(123), 'ValueError.__new__(X): X is not a type object (int)'),
    (
        lambda: ValueError.__new__(TypeError),
        'ValueError.__new__(TypeError): TypeError is not a subtype of ValueError',
    ),
    (
        lambda: ValueError.__new__(Bare),
        'ValueError.__new__(Bare): Bare is not a subtype of ValueError',
    ),
]:
    try:
        bad_call()
        assert False, f'expected TypeError: {expected_msg}'
    except TypeError as err:
        assert str(err) == expected_msg, f'{err!r} != {expected_msg!r}'


# === Extended __new__, __init__, __init_subclass__, and custom __new__ instantiation ===
from collections import namedtuple
from dataclasses import dataclass
import decimal

NewPoint = namedtuple('NewPoint', ['x', 'y'])

for bad_call, expected_msg in [
    (lambda: object.__new__(), 'object.__new__(): not enough arguments'),
    (
        lambda: object.__new__(decimal.DivisionByZero),
        'object.__new__(DivisionByZero) is not safe, use DivisionByZero.__new__()',
    ),
    (lambda: object.__new__(int), 'object.__new__(int) is not safe, use int.__new__()'),
    (lambda: object.__new__(type), 'object.__new__(type) is not safe, use type.__new__()'),
    (lambda: object.__new__(NewPoint), 'object.__new__(NewPoint) is not safe, use tuple.__new__()'),
    (lambda: object.__new__([1, 2]), 'object.__new__(X): X is not a type object (list)'),
    (lambda: object.__new__(42), 'object.__new__(X): X is not a type object (int)'),
    (
        lambda: ValueError.__new__(NewPoint),
        'ValueError.__new__(NewPoint): NewPoint is not a subtype of ValueError',
    ),
    (
        lambda: ValueError.__new__([1, 2]),
        'ValueError.__new__(X): X is not a type object (list)',
    ),
    (
        lambda: ValueError.__new__(int),
        'ValueError.__new__(int): int is not a subtype of ValueError',
    ),
    (
        lambda: ValueError.__new__(type),
        'ValueError.__new__(type): type is not a subtype of ValueError',
    ),
    (
        lambda: object.__init__(),
        "descriptor '__init__' of 'object' object needs an argument",
    ),
    (
        lambda: Bare.__init__(Bare(), 1),
        'Bare.__init__() takes exactly one argument (the instance to initialize)',
    ),
    (
        lambda: object.__init__(CustomErr('a'), 1),
        'object.__init__() takes exactly one argument (the instance to initialize)',
    ),
    (
        lambda: object.__init__(CustomErr('a'), k=1),
        'object.__init__() takes exactly one argument (the instance to initialize)',
    ),
    (
        lambda: ValueError.__init__(),
        "descriptor '__init__' of 'BaseException' object needs an argument",
    ),
    (
        lambda: ValueError.__init__(42),
        "descriptor '__init__' requires a 'BaseException' object but received a 'int'",
    ),
    (
        lambda: ValueError.__init__(Bare()),
        "descriptor '__init__' requires a 'BaseException' object but received a 'Bare'",
    ),
    (
        lambda: Bare.__init_subclass__(1),
        'Bare.__init_subclass__() takes no arguments (1 given)',
    ),
    (
        lambda: Bare.__init_subclass__(k=1),
        'Bare.__init_subclass__() takes no keyword arguments',
    ),
    (
        lambda: int.__init_subclass__(1),
        'int.__init_subclass__() takes no arguments (1 given)',
    ),
    (
        lambda: ValueError.__init_subclass__(1),
        'ValueError.__init_subclass__() takes no arguments (1 given)',
    ),
    (
        lambda: type.__init_subclass__(1),
        'type.__init_subclass__() takes no arguments (1 given)',
    ),
]:
    try:
        bad_call()
        assert False, f'expected TypeError: {expected_msg}'
    except TypeError as err:
        assert str(err) == expected_msg, f'{err!r} != {expected_msg!r}'

# Direct builtin exception allocation via __new__ and re-init via ValueError.__init__
be_ve = ValueError.__new__(ValueError)
assert isinstance(be_ve, ValueError) and be_ve.args == ()
be_sub = Exception.__new__(ValueError)
assert isinstance(be_sub, ValueError) and be_sub.args == ()
ValueError.__init__(be_sub, 'updated')
assert be_sub.args == ('updated',)
ValueError.__init__(be_sub)
assert be_sub.args == ()
assert isinstance(ValueError.__new__(CustomErr, k=1), CustomErr)


class ReturnsList:
    def __new__(cls, x):
        return [x, x + 1]

    def __init__(self, x):
        assert False


assert ReturnsList(5) == [5, 6]


@dataclass
class DcWithNew:
    a: int
    b: str = 'ok'

    def __new__(cls, *args, **kwargs):
        inst = object.__new__(cls)
        inst.from_new = True
        return inst


dwn = DcWithNew(7)
assert dwn.a == 7 and dwn.b == 'ok' and dwn.from_new is True


class ExcWithNew(ValueError):
    def __new__(cls, *args, **kwargs):
        return ValueError.__new__(cls, *args)


assert ExcWithNew().args == ()
assert ExcWithNew('a').args == ('a',)
assert ExcWithNew('a', 'b').args == ('a', 'b')
assert ExcWithNew('a', 'b', 'c').args == ('a', 'b', 'c')
for bad_kw in [lambda: ExcWithNew(k=1), lambda: ExcWithNew('a', k=1)]:
    try:
        bad_kw()
        assert False, 'expected TypeError'
    except TypeError as err:
        assert str(err) == 'ExcWithNew() takes no keyword arguments'
