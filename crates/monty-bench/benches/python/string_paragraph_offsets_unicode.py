# /// monty
# [bench]
# data = { corpus = 'mostly-ascii', chars = 131072 }
# ///
# Index paragraph boundaries for chunking or highlighting a 128K-character document (~99% ASCII).
length = len(DATA)
start = 0
boundaries = []

while start < length:
    end = DATA.find('\n\n', start)
    if end < 0:
        end = length
    boundaries.append((start, end))
    start = end + 2

assert len(boundaries) == 687
assert sum(end - start for start, end in boundaries) == 129700
assert sum(start for start, end in boundaries) == 44973213
