# /// monty
# [bench]
# data = { corpus = 'mostly-ascii', chars = 6472 }
# ///
# Scan generated ~99% ASCII prose as a basic text scanner would.
checksum = 0
word_count = 0

for size in (512, 2048, len(DATA)):
    text = DATA[:size]
    in_word = False

    for index in range(len(text)):
        char = text[index]
        checksum += ord(char)

        if char.isspace():
            in_word = False
        elif not in_word:
            word_count += 1
            in_word = True

assert (word_count, checksum) == (1465, 842630)
