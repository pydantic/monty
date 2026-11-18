total = 0
for i in range(300_000):
    total = (total + i * 3) % 1_000_003
assert total == 145003
