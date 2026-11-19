# Appends 100,000 ints, isolating `list.append`.

a = []
for i in range(100_000):
    a.append(i)
assert sum(a) == 4_999_950_000
