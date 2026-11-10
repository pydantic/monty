def hot():
    a = 1
    b = 2
    for _ in range(100_000):
        a, b = b, a
    return a * 10 + b
assert hot() == 12
