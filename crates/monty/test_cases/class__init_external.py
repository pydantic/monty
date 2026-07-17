# call-external
# An __init__ that suspends on an external call. Because __init__ runs as a real
# frame, the VM yields to the host mid-construction and resumes — exercising the
# initializer frame (and its is_initializer flag) across the suspend point.


class Accumulator:
    def __init__(self, base: int) -> None:
        # `add_ints` is an external function resolved by the host.
        self.total = add_ints(base, 100)
        self.base = base

    def bump(self, amount: int) -> int:
        self.total = add_ints(self.total, amount)
        return self.total


a = Accumulator(5)
assert a.base == 5
assert a.total == 105
assert type(a) is Accumulator

# A method that also suspends on an external call resumes correctly.
assert a.bump(10) == 115
assert a.total == 115
