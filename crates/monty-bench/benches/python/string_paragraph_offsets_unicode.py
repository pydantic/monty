# /// monty
# [bench]
# data = '../data/string_paragraphs_128k.txt'
# ///
# Index paragraph boundaries for chunking or highlighting a 128K-character document (~99.12% ASCII).
# Normalize line endings so the offsets also match on Windows checkouts.
text = DATA.replace('\r\n', '\n')
length = len(text)
start = 0
boundaries = []

while start < length:
    end = text.find('\n\n', start)
    if end < 0:
        end = length
    boundaries.append((start, end))
    start = end + 2

assert len(boundaries) == 507
assert sum(end - start for start, end in boundaries) == 130060
assert sum(start for start, end in boundaries) == 33171634
