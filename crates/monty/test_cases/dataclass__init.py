# Native `@dataclass` construction: the synthesized `__init__` binds positional
# and keyword arguments to the annotated fields, applies defaults, excludes
# `ClassVar`, and raises CPython's exact arity/keyword errors.
import typing
from dataclasses import dataclass
from typing import ClassVar


@dataclass
class Point:
    x: int
    y: int


# === Construction: positional, keyword, mixed ===
p = Point(1, 2)
assert p.x == 1 and p.y == 2, 'positional construction sets fields'
assert Point(10, y=20).y == 20
assert Point(x=5, y=6).x == 5


# === Defaults ===
@dataclass
class WithDefault:
    a: int
    b: int = 5
    c: str = 'hi'


assert WithDefault(1).b == 5
assert WithDefault(1).c == 'hi'
assert WithDefault(1, 2).b == 2
assert WithDefault(1, c='x').c == 'x'
assert WithDefault(1, b=9).b == 9


# === ClassVar is excluded from the constructor ===
@dataclass
class WithClassVar:
    x: int
    count: ClassVar[int] = 0


assert WithClassVar(7).x == 7
assert WithClassVar.count == 0


# A dotted spelling is excluded too, and a dotted *type argument*
# (`ClassVar[typing.Dict[...]]`) must not confuse the qualifier match. The
# quoted spelling is covered in `tests/dataclass_rejections.rs`: CPython
# resolves it via the defining module's namespace, so it is not stable here.
@dataclass
class ClassVarSpellings:
    x: int
    bare: ClassVar[int] = 1
    dotted: typing.ClassVar[int] = 3
    dotted_arg: ClassVar[typing.Dict[str, int]] = {}


assert repr(ClassVarSpellings(7)) == 'ClassVarSpellings(x=7)'
# Both stay class attributes — and a ClassVar, being no field, may be mutable.
assert ClassVarSpellings.dotted == 3
assert ClassVarSpellings.dotted_arg == {}


# === Errors: messages match CPython exactly ===
def expect_type_error(fn, message):
    try:
        fn()
        assert False, f'expected TypeError: {message}'
    except TypeError as e:
        assert str(e) == message, f'wrong message: {e!r}'


expect_type_error(lambda: Point(), "Point.__init__() missing 2 required positional arguments: 'x' and 'y'")
expect_type_error(lambda: Point(1), "Point.__init__() missing 1 required positional argument: 'y'")
expect_type_error(lambda: Point(1, 2, 3), 'Point.__init__() takes 3 positional arguments but 4 were given')
expect_type_error(lambda: Point(1, x=2), "Point.__init__() got multiple values for argument 'x'")
expect_type_error(lambda: Point(1, 2, z=3), "Point.__init__() got an unexpected keyword argument 'z'")
expect_type_error(lambda: WithClassVar(7, 8), 'WithClassVar.__init__() takes 2 positional arguments but 3 were given')

# A non-string `**kwargs` key is raised by the call machinery before `__init__`
# is entered, so it outranks every arity error.
expect_type_error(lambda: Point(**{1: 2}), 'keywords must be strings')
expect_type_error(lambda: Point(**{1: 2, 'x': 5}), 'keywords must be strings')
# Every key is checked before any is bound, so a non-string key wins even when
# an unbindable one comes first.
expect_type_error(lambda: Point(**{'z': 1, 1: 2}), 'keywords must be strings')
expect_type_error(lambda: Point(1, **{'x': 2, 1: 3}), 'keywords must be strings')
expect_type_error(lambda: Point(1, 2, 3, **{1: 2}), 'keywords must be strings')


# === Class-body rejections that match CPython exactly ===
def expect_error(fn, exc_type, message):
    try:
        fn()
        assert False, f'expected an exception: {message}'
    except exc_type as e:
        assert str(e) == message, f'wrong message: {e!r}'


def mutable_default():
    @dataclass
    class BadList:
        xs: list[int] = []


def mutable_default_dict():
    @dataclass
    class BadDict:
        d: dict[str, int] = {}


def non_default_after_default():
    @dataclass
    class BadOrder:
        a: int = 1
        b: int


# CPython's rule is hashability; Monty matches it for built-in unhashable types.
# A class setting `__hash__ = None` is NOT rejected (see limitations/dataclasses.md).
expect_error(
    mutable_default, ValueError, "mutable default <class 'list'> for field xs is not allowed: use default_factory"
)
expect_error(
    mutable_default_dict, ValueError, "mutable default <class 'dict'> for field d is not allowed: use default_factory"
)
expect_error(non_default_after_default, TypeError, "non-default argument 'b' follows default argument 'a'")


# === Keyword binding is reported before the positional-arity error ===
# CPython's generated `__init__` binds keywords first, so an excess positional
# only surfaces once the keywords are known to be valid.
expect_type_error(lambda: Point(1, 2, 3, z=4), "Point.__init__() got an unexpected keyword argument 'z'")
expect_type_error(lambda: Point(1, 2, 3, x=4), "Point.__init__() got multiple values for argument 'x'")
expect_type_error(lambda: WithDefault(1, 2, 3, 4, b=9), "WithDefault.__init__() got multiple values for argument 'b'")
expect_type_error(
    lambda: WithDefault(1, 2, 3, 4), 'WithDefault.__init__() takes from 2 to 4 positional arguments but 5 were given'
)


# === Defaults are captured when @dataclass runs, not read at construction ===
# CPython bakes them into the generated `__init__`, so rebinding the class
# attribute afterwards changes `Rebind.b` but not what new instances receive.
@dataclass
class Rebind:
    a: int
    b: int = 5


Rebind.b = 99
assert Rebind(1).b == 5
assert Rebind.b == 99
assert repr(Rebind(1)) == 'Rebind(a=1, b=5)'
