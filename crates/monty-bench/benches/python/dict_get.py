mapping = {'hit': 3}
for _ in range(10_000):
    hit = mapping.get('hit')
    miss = mapping.get('miss', 4)
assert hit == 3 and miss == 4
