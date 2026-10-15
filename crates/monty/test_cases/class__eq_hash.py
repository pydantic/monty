# User-defined `__eq__` / `__hash__` on a class are dispatched, including the
# CPython rule that defining `__eq__` makes instances unhashable unless the
# class also defines `__hash__`. All behaviour here matches CPython.
from dataclasses import dataclass


class Eq:
    def __init__(self, v: int) -> None:
        self.v = v

    def __eq__(self, other: object) -> bool:
        return isinstance(other, Eq) and self.v == other.v


# === __eq__ is dispatched, and != is derived from it ===
assert Eq(1) == Eq(1)
assert not (Eq(1) == Eq(2)), 'differing values are unequal'
assert Eq(1) != Eq(2)
assert not (Eq(1) != Eq(1)), '!= is the negation of __eq__'

# A non-matching type takes the user's own `isinstance` branch.
assert not (Eq(1) == 1), 'comparison against a foreign type'
assert Eq(1) != 1

# === Defining __eq__ alone makes instances unhashable ===
try:
    hash(Eq(1))
    assert False, 'expected unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'Eq'", f'wrong message: {e!r}'


class EqHash:
    def __init__(self, v: int) -> None:
        self.v = v

    def __eq__(self, other: object) -> bool:
        return isinstance(other, EqHash) and self.v == other.v

    def __hash__(self) -> int:
        return hash(self.v)


# === __hash__ alongside __eq__ restores hashability ===
assert hash(EqHash(1)) == hash(EqHash(1))
assert hash(EqHash(1)) == hash(1)

# === Equal + hashable objects collapse in sets and match as dict keys ===
assert len({EqHash(1), EqHash(1)}) == 1
assert len({EqHash(1), EqHash(2)}) == 2
assert {EqHash(1): 'x'}[EqHash(1)] == 'x'
assert EqHash(1) in {EqHash(1): 'x'}

# === __eq__ drives containment and equality of containers ===
assert Eq(1) in [Eq(2), Eq(1)]
assert Eq(3) not in [Eq(2), Eq(1)]
assert [Eq(1), Eq(2)] == [Eq(1), Eq(2)]
assert (Eq(1),) == (Eq(1),)


# === __hash__ = None opts out explicitly ===
class NoHash:
    __hash__ = None


try:
    hash(NoHash())
    assert False, 'expected unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'NoHash'", f'wrong message: {e!r}'


# === A class with neither dunder keeps identity semantics ===
class Plain:
    def __init__(self, v: int) -> None:
        self.v = v


p = Plain(1)
assert p == p
assert not (Plain(1) == Plain(1)), 'distinct instances are unequal without __eq__'
assert isinstance(hash(p), int), 'hashable by identity'


# === A dataclass body may define __eq__, and it wins over the synthesized one ===
@dataclass
class DC:
    x: int

    def __eq__(self, other: object) -> bool:
        return True


assert DC(1) == DC(2)
try:
    hash(DC(1))
    assert False, 'expected unhashable'
except TypeError as e:
    assert str(e) == "unhashable type: 'DC'", f'wrong message: {e!r}'


# === A dataclass with no user __eq__ still gets field-wise equality ===
@dataclass
class Synth:
    x: int


assert Synth(1) == Synth(1)
assert Synth(1) != Synth(2)


# === A user __eq__ decides `x == x` too; containers still shortcut on identity ===
# CPython's `==` operator dispatches `__eq__` even for the same object, while
# container membership/comparison uses `PyObject_RichCompareBool`, which treats
# identical objects as equal without consulting `__eq__`.
class Never:
    def __eq__(self, other: object) -> bool:
        return False

    def __hash__(self) -> int:
        return 1


never = Never()
assert not (never == never), 'the operator dispatches __eq__ against self'
assert never != never
assert never in [never]
assert [never] == [never]
assert (never,) == (never,)
assert {'k': never} == {'k': never}
assert [never].index(never) == 0


# A synthesized dataclass `__eq__` opens with `if self is other: return True`,
# so identity wins there — but each *field* is compared with the operator.
@dataclass
class Holder:
    f: object


holder = Holder(never)
assert holder == holder
assert not (Holder(never) == Holder(never)), 'fields compare with the == operator'
