n = 1
for i in range(1, 1200):
    n *= i
assert len(str(n)) == 3173
assert n % 1009 == 0
m = 3**20_000
assert m % 10 == 1
assert (m * m) // m == m
assert pow(7, 10**6, 10**40 + 7) > 0
