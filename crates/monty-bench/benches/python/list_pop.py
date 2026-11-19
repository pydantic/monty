items = [1, 2]
for _ in range(10_000):
    items.append(items.pop())
    items.append(items.pop(0))
assert items == [1, 2]
