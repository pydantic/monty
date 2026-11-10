def hot():
    total = 0
    for i in range(100_000):
        if i > 50_000:
            total += 1
        if i <= 50_000:
            total += 2
        if i == 99_999:
            total += 3
    return total
assert hot() == 150_004
