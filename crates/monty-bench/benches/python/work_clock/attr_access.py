class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def shift(self, dx):
        self.x += dx
        return self.y


p = Point(0, 2)
total = 0
for i in range(60_000):
    total += p.shift(1) + p.x
assert total == 1800150000
