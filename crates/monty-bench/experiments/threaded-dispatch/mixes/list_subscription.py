def hot():
    values = [1, 2, 3]
    total = 0
    for _ in range(100_000):
        total += values[0] + values[1] + values[2]
    return total
assert hot() == 600_000
