# `@dataclass` captures each field's default at decoration time, so the `Field`
# in `__dataclass_fields__` holds an owned reference alongside the class
# namespace's own. After decoration `shared` is referenced by the module global,
# the class namespace entry `Defaults.b`, and the `Field`. `nested` is
# referenced by its own module global and by `shared`, and by nothing the
# decorator adds — capturing a default clones the reference to `shared` itself,
# not those inside it.
from dataclasses import dataclass

nested = (3, 4)
shared = (1, 2, nested)


@dataclass
class Defaults:
    a: int
    b: tuple[int, ...] = shared


# Constructing clones the default into the instance, which is dropped here.
assert Defaults(1).b == shared

# ref-counts={'nested': 2, 'shared': 3, 'Defaults': 1}
