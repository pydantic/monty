# `@dataclass` captures each field's default at decoration time, so the metadata
# holds an owned reference alongside the class namespace's own. After decoration
# `shared` is referenced by the module global, the class namespace entry
# `Defaults.b`, and the captured default. `nested` is reachable only through
# `shared`, so it keeps a single reference.
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
