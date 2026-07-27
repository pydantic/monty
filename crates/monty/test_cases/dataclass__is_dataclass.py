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
assert Point.__name__ == 'Point', 'the decorator returns the same class'
