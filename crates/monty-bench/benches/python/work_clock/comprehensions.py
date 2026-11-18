squares = [i * i for i in range(120_000)]
evens = [s for s in squares if s % 2 == 0]
assert len(evens) == 60_000
lookup = {i: s for i, s in enumerate(squares) if i % 3 == 0}
assert len(lookup) == 40_000
total = sum(s % 7 for s in squares)
assert total == 240001
grid = [[x * y for x in range(60)] for y in range(600)]
assert sum(len(row) for row in grid) == 36_000
assert any(row[-1] == 59 * 599 for row in grid)
