# Classes defined inside functions: methods skip the class scope for name
# resolution but can still capture enclosing *function* locals (matching CPython).


def make_multiplier(n: int):
    class Multiplier:
        def __init__(self, base: int) -> None:
            self.base = base

        def compute(self) -> int:
            # `n` is captured from the enclosing function, not the class body.
            return self.base * n

    return Multiplier(10)


m = make_multiplier(3)
assert m.compute() == 30, 'method captures enclosing function local'
assert m.base == 10, 'instance attribute set in nested class __init__'

m2 = make_multiplier(5)
assert m2.compute() == 50, 'each closure captures its own enclosing value'
assert m.compute() == 30, 'independent closures do not interfere'


def counter_factory():
    count = 0

    class Counter:
        def bump(self) -> int:
            nonlocal count
            count += 1
            return count

    return Counter()


c = counter_factory()
assert c.bump() == 1, 'nonlocal captured cell shared with method'
assert c.bump() == 2, 'cell mutation persists across method calls'
assert c.bump() == 3, 'cell keeps accumulating'


# A class defined at module scope still resolves globals (not class members) by
# bare name from inside methods.
FACTOR = 4


class Scaled:
    def __init__(self, v: int) -> None:
        self.v = v

    def scale(self) -> int:
        return self.v * FACTOR


assert Scaled(3).scale() == 12, 'method reads a module global by bare name'
