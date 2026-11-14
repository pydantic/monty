def hot():
    a = 17
    b = 23
    for _ in range(100_000):
        a = b
        b = a
    return a + b


assert hot() == 46
