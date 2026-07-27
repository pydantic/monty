# `@dataclass` captures each field's default at decoration time, so the metadata
# on the `Class` holds an owned reference alongside the class namespace's own.
#
# After decoration `shared` is referenced by:
#   - the module global `shared`
#   - the class namespace entry `Defaults.b` (the annotated assignment)
#   - the captured default in the class's dataclass metadata
#
# `nested` is only reachable through `shared`, so it keeps a single reference.
from dataclasses import dataclass

nested = (3, 4)
shared = (1, 2, nested)


@dataclass
class Defaults:
    a: int
    b: tuple[int, ...] = shared


# Constructing does not capture again — the default is cloned into the instance,
# which is dropped here, so the counts are unchanged by this line.
assert Defaults(1).b == shared

# ref-counts={'nested': 2, 'shared': 3, 'Defaults': 1}
