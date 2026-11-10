class Record:
    x = 1
def hot():
    r = Record()
    total = 0
    for _ in range(100_000):
        total += r.x
    return total
result = hot()
print(result)
