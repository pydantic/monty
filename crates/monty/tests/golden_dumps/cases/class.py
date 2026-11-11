class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def total(self):
        return self.x + self.y


p = Point(3, 4)
same = p
total = p.total()
