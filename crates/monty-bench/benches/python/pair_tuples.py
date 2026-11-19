# Creates 100,000 2-tuples in a list.

assert len([(i, i + 1) for i in range(100_000)]) == 100_000
