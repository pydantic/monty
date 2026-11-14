# === slice.indices(length) ===
s1 = slice(1, 10, 2)
assert s1.indices(5) == (1, 5, 2)

s2 = slice(None, None, -1)
assert s2.indices(5) == (4, -1, -1)

s3 = slice(-100, 100)
assert s3.indices(10) == (0, 10, 1)

s4 = slice(10, -10, -2)
assert s4.indices(5) == (4, -1, -2)

s5 = slice(None, None, None)
assert s5.indices(5) == (0, 5, 1)

s6 = slice(-2, -5, -1)
assert s6.indices(5) == (3, 0, -1)

s7 = slice(None, -1, 1)
assert s7.indices(5) == (0, 4, 1)

s8 = slice(-10, None, -1)
assert s8.indices(5) == (-1, -1, -1)

s9 = slice(5, 5, 1)
assert s9.indices(5) == (5, 5, 1)

s10 = slice(5, 5, -1)
assert s10.indices(5) == (4, 4, -1)

s11 = slice(-5, -5, 1)
assert s11.indices(5) == (0, 0, 1)

s12 = slice(100, -100, -1)
assert s12.indices(5) == (4, -1, -1)

# Zero length sequence
assert slice(-100, 100, 1).indices(0) == (0, 0, 1)
assert slice(None, None, -1).indices(0) == (-1, -1, -1)
assert slice(0, 0, 1).indices(0) == (0, 0, 1)
assert slice(-1, -1, -1).indices(0) == (-1, -1, -1)
assert slice(10, 20, 3).indices(0) == (0, 0, 3)

# Boolean length
assert slice(1, 10, 2).indices(True) == (1, 1, 2)
assert slice(1, 10, 2).indices(False) == (0, 0, 2)


# Custom class with __index__
class CustomIndex:
    def __init__(self, val):
        self.val = val

    def __index__(self):
        return self.val


assert slice(1, 10, 2).indices(CustomIndex(5)) == (1, 5, 2)

# Using indices in range() matches sequence slicing
seq = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
for sl in [
    slice(1, 8, 2),
    slice(8, 1, -2),
    slice(-7, -2),
    slice(-2, -7, -1),
    slice(None, None, 3),
    slice(None, None, -2),
]:
    start, stop, step = sl.indices(len(seq))
    assert [seq[i] for i in range(start, stop, step)] == seq[sl]
