# A user iterator must not leak. `list(c)` drives `c` directly — no wrapper is
# allocated to hold a reference to it — so once iteration finishes the instance
# is back to just its own binding. `Countdown` is 2: the global name plus the
# instance's class slot.
class Countdown:
    def __init__(self, n):
        self.n = n

    def __iter__(self):
        return self

    def __next__(self):
        if self.n <= 0:
            raise StopIteration
        self.n -= 1
        return self.n + 1


c = Countdown(3)
out = list(c)
out
# ref-counts={'c': 1, 'out': 2, 'Countdown': 2}
