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


c = Countdown(3)
out = bytes(c)
out
# ref-counts={'c': 1, 'out': 2, 'Countdown': 2}
