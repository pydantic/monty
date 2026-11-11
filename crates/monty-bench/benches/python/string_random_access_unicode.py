# /// monty
# [bench]
# data = { corpus = 'multilingual', chars = 4653, offsets = 256 }
# ///
# Text and seeded random offsets are generated once before timing.
checksum = 0

for size in (512, 2048, len(DATA)):
    text = DATA[:size]
    length = len(text)

    for offset in OFFSETS:
        checksum += ord(text[offset % length])

assert checksum == 715622
