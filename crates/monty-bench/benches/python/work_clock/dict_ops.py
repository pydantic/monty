counts = {}
for i in range(80_000):
    key = i % 5000
    counts[key] = counts.get(key, 0) + 1
assert len(counts) == 5000
assert sum(counts.values()) == 80_000
names = {f'k{i}': i for i in range(20_000)}
hits = 0
for i in range(0, 40_000, 2):
    if f'k{i}' in names:
        hits += 1
assert hits == 10_000
seen = set()
for i in range(60_000):
    seen.add(i * 31 % 9973)
assert len(seen) == 9973
