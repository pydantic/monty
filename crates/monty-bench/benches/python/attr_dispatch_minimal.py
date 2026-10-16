# Several attribute names with cheap, non-allocating method bodies: operations on an
# empty list, a one-entry dict and a single-character string.

xs = []
d = {'k': 1}
s = 'x'
r = 0
for _ in range(10_000):
    xs.clear()
    xs.reverse()
    r += d.get('k', 0)
    r += 1 if s.startswith('x') else 0
    r += 1 if s.endswith('x') else 0
assert r == 30_000
