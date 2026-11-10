def hot():
    d = {1: 1, 2: 2, 3: 3}
    total = 0
    for _ in range(100_000):
        total += d[1] + d[2] + d[3]
    return total
result = hot()
print(result)
