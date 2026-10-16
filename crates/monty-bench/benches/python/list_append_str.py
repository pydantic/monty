# Appends 100,000 freshly allocated strings.

a = []
for i in range(100_000):
    a.append(str(i))
assert len(a) == 100_000
