# A captured dataclass default can reach back to the class that captured it,
# making `__dataclass_fields__` part of a reference cycle:
#
#   Cyclic  ->  Field `b`  ->  default `box`  ->  box.owner  ->  Cyclic
#
# The cycle collector must therefore walk a `Field`'s captured default as its
# child. An instance is an accepted default because dataclasses reject them by
# *hashability*, and a class defining no `__eq__`/`__hash__` hashes by identity.
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

# `box`: the global, the class namespace entry `Cyclic.b`, and the `Field`'s
# captured default. `Cyclic`: the global plus `box.owner`. `Box`: the global plus the
# class reference every instance holds.
# ref-counts={'box': 3, 'Cyclic': 2, 'Box': 2}
