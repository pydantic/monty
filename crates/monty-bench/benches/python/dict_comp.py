# A dict comprehension creating 500 unique keys (`i // 2` deduplicates pairs).

assert len({i // 2: i * 2 for i in range(1000)}) == 500
