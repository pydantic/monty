def hot():
    values = list(range(50))
    total = 0
    for _ in range(2_000):
        for value in values:
            total += value
    return total
assert hot() == 2_450_000
