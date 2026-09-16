# The `field(...)` spec is adopted at decoration: the same object goes into
# `__dataclass_fields__` and keeps owning what it captured, while the class
# namespace loses its binding to it. `Item` is a heap-allocated class used as
# the factory, referenced by its module global and by the field that captured
# it; `shared` by its global, that field's default, and the class attribute
# that replaced the spec.
from dataclasses import dataclass, field


class Item:
    pass


shared = (1, 2)


@dataclass
class C:
    it: Item = field(default_factory=Item)
    t: tuple[int, int] = field(default=shared)


# Constructing calls the factory and clones the default; both are dropped here.
assert isinstance(C().it, Item)
assert C().t == shared

# ref-counts={'Item': 2, 'shared': 3, 'C': 1}
