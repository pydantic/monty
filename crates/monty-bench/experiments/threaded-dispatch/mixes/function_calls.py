def add(x):
    return x + 1


def hot():
    total = 0
    for _ in range(50_000):
        total = add(total)
    return total


assert hot() == 50_000
