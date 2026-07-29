# Native `@dataclass`: the decorator marks a class as a dataclass, and
# `is_dataclass` recognizes both the class and its instances. Field-based
# construction (`Point(1, 2)`) arrives in a later phase, so this file only
# constructs the field-less `Empty` (which needs no constructor arguments on
# either interpreter).
from dataclasses import dataclass, is_dataclass


@dataclass
class Point:
    x: int
    y: int


@dataclass
class Empty:
    pass


class Plain:
    pass


# === is_dataclass on classes ===
assert is_dataclass(Point), 'decorated class is a dataclass'
assert is_dataclass(Empty), 'empty decorated class is a dataclass'
assert not is_dataclass(Plain), 'plain (undecorated) class is not a dataclass'
assert not is_dataclass(int), 'a builtin type is not a dataclass'
assert not is_dataclass(5), 'a non-class value is not a dataclass'
assert not is_dataclass('hi'), 'a string is not a dataclass'

# === is_dataclass on an instance ===
e = Empty()
assert is_dataclass(e), 'an instance of a dataclass is itself a dataclass'

# === The decorated class is unchanged as a class object ===
assert Point.__name__ == 'Point'

# === __dataclass_fields__ ===
# Decoration records a `name -> Field` mapping on the class, in definition order.
assert list(Point.__dataclass_fields__) == ['x', 'y']
assert list(Empty.__dataclass_fields__) == []
assert not hasattr(Plain, '__dataclass_fields__'), 'a plain class has no field mapping'
# Instances read it through the class, so both see the same object.
assert e.__dataclass_fields__ is Empty.__dataclass_fields__


@dataclass
class Defaulted:
    a: int
    b: str = 'hi'


# === Field objects ===
b = Defaulted.__dataclass_fields__['b']
assert type(b).__name__ == 'Field'
assert b.name == 'b'
assert b.default == 'hi'
# Monty stores annotations as source text where CPython evaluates them.
assert b.type in ('str', str)
# Every field Monty builds carries the decorator defaults: `field()` and
# `@dataclass(...)`, the forms that vary these, are not supported.
assert b.init is True
assert b.repr is True
assert b.compare is True
assert b.kw_only is False
assert b.hash is None
# `Field.doc` is new in CPython 3.14, so a pre-3.14 dual-run has no attribute to
# compare; Monty always has it (see `tests/dataclass_fields.rs` for the strict check).
if hasattr(b, 'doc'):
    assert b.doc is None

try:
    b.nope
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'Field' object has no attribute 'nope'"
