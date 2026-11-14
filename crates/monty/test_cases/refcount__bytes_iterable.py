class Countdown:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        return self

    def __next__(self):
        if self.n <= 0:
            raise StopIteration
        self.n -= 1
        return 65


class FailingIter:
    def __init__(self):
        self.step = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.step += 1
        if self.step == 1:
            return 65
        return 256


c = Countdown(3)
out = bytes(c)

bad = FailingIter()
try:
    bytes(bad)
except ValueError:
    pass

out
# ref-counts={'c': 1, 'out': 2, 'Countdown': 2, 'bad': 1, 'FailingIter': 2}
