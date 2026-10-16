# Creates 100,000 empty tuples in a list.

assert len([() for _ in range(100_000)]) == 100_000
