# /// monty
# [bench]
# data = '../data/string_mostly_ascii.txt'
# ///
# The document is 99.12% ASCII by code point, with irregular Unicode names and punctuation.
# Index through the text and count word boundaries, as in a basic text scanner.
# Normalize fixture line endings so checksums also work with Windows checkouts.
data = DATA.replace('\r\n', '\n')
checksum = 0
word_count = 0

for size in (512, 2048, len(data)):
    text = data[:size]
    in_word = False

    for index in range(len(text)):
        char = text[index]
        checksum += ord(char)

        if char.isspace():
            in_word = False
        elif not in_word:
            word_count += 1
            in_word = True

assert (word_count, checksum) == (1457, 1135407)
