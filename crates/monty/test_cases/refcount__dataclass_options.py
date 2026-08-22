# `dataclass(...)` takes only the truthiness of each option, so a heap value
# passed as one must be released rather than held by the decorator. `flag` is
# referenced by its module global alone once the call returns — both for an
# option Monty implements (`eq`) and for one it only recognises (`match_args`,
# passed at its default so the call is accepted).
from dataclasses import dataclass

flag = (1,)


@dataclass(eq=flag, match_args=flag)
class Options:
    a: int


assert Options(1) == Options(1)

# The configured decorator holds no heap reference (hence no `deco` count
# below), so the same holds when it is bound to a name before being applied.
deco = dataclass(frozen=flag)


@deco
class Frozen:
    a: int


assert hash(Frozen(1)) == hash((1,))

# ref-counts={'flag': 1, 'Options': 1, 'Frozen': 1}
