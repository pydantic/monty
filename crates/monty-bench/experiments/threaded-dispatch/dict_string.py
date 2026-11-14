def hot():
    d = {'x': 1, 'y': 2, 'z': 3}
    total = 0
    for _ in range(100_000):
        total += d['x'] + d['y'] + d['z']
    return total


result = hot()
print(result)
