# The `field(...)` spec is unwrapped at decoration and then cleared out of the
# class namespace, so the spec object itself keeps nothing alive. `Item` is a
# heap-allocated class used as the factory, referenced by its module global and
# by the metadata that captured it; `shared` by its global, the metadata, and
# the class attribute that replaced the spec.
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
