sequence = 'aé中🐍' * 2500
total = 0
for _ in range(10_000):
    total += ord(sequence[0]) + ord(sequence[-1])
assert total == 1_281_100_000
