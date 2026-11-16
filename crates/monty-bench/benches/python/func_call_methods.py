# Small positional calls through direct methods, cached methods and constructors.


class Pair:
    def __init__(self, first, second):
        self.first = first
        self.second = second

    def receiver(self):
        return self

    def pair(self, first, second):
        return second

    def eight(self, a, b, c, d, e, f, g, h):
        return h


def benchmark(count):
    first = [1]
    second = [2]
    obj = Pair(first, second)
    cached = obj.pair
    for _ in range(count):
        receiver = obj.receiver()
        direct = obj.pair(first, second)
        many = obj.eight(first, second, first, second, first, second, first, second)
        bound = cached(first, second)
        constructed = Pair(first, second)

    assert receiver is obj
    assert direct is many is bound is second
    assert constructed.first is first
    assert constructed.second is second


benchmark(20000)
