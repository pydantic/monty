def hot():
    total = 0
    for i in range(50_000):
        try:
            if i % 100 == 0:
                raise ValueError('expected')
            total += 1
        except ValueError:
            total += 2
    return total
assert hot() == 50_500
