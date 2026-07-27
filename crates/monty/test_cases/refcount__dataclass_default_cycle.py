# A captured dataclass default can reach back to the class that captured it,
# making the class's metadata part of a reference cycle:
#
#   Cyclic  ->  metadata default `box`  ->  box.owner  ->  Cyclic
#
# The cycle collector must therefore walk the captured defaults as children of
# the class, the same way it walks the class namespace. An instance default is
# accepted because dataclasses reject defaults by *hashability*, and a class
# defining neither `__eq__` nor `__hash__` hashes by identity.
from dataclasses import dataclass


class Box:
    def __init__(self) -> None:
        self.owner = None


box = Box()


@dataclass
class Cyclic:
    a: int
    b: object = box


box.owner = Cyclic

assert Cyclic(1).b is box
assert Cyclic(1).b.owner is Cyclic

# `box`: the global, the class namespace entry `Cyclic.b`, and the captured
# default. `Cyclic`: the global plus `box.owner`. `Box`: the global plus the
# class reference every instance holds.
# ref-counts={'box': 3, 'Cyclic': 2, 'Box': 2}
