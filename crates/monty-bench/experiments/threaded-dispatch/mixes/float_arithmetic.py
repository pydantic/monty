def hot():
    x = 1.25
    total = 0.0
    for _ in range(100_000):
        total += x * 2.0 - 0.5
    return total


assert hot() == 200_000.0
