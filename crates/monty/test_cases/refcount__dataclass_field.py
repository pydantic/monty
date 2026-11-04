# The `field(...)` spec is adopted at decoration: the same object goes into
# `__dataclass_fields__` and keeps owning what it captured, while the class
# namespace loses its binding to it. `Item` is a heap-allocated class used as
# the factory and as the field's annotation, referenced by its module global,
# the field that captured it (as `default_factory` and as `type`), the
# generated `__init__` and the class's cached `__annotations__`; `shared` by
# its global, that field's default, `__init__`'s copy of it, and the class
# attribute that replaced the spec.
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

# ref-counts={'Item': 5, 'shared': 4, 'C': 1}
