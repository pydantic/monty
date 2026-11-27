# Keep each argument alive across calls so the benchmark measures len and dispatch.
items = [1, 2, 3]
coords = (1, 2)
text = 'abcd'

total = 0
for _ in range(10_000):
    total += len(items) + len(coords) + len(text)
assert total == 90_000
