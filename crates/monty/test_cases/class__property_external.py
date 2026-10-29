# /// monty
# call-external = true
# ///
# A property getter read as `obj.attr` runs as a real frame, so it may suspend
# on an external call; `add_ints` is resolved by the host.


class Config:
    def __init__(self, base):
        self.base = base

    @property
    def limit(self):
        return add_ints(self.base, 2)

    @staticmethod
    def doubled(x):
        return add_ints(x, x)

    @classmethod
    def make(cls):
        return cls(add_ints(20, 20))


assert Config(40).limit == 42
assert Config.doubled(21) == 42
assert Config.make().limit == 42
