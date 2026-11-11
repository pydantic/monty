# /// monty
# [bench]
# data = '../data/string_corpus.json'
# ///
import json

# Corpus sampling and text construction are part of the timed execution.
corpus = json.loads(DATA)
state = 42
parts = []
characters = 0

while characters < 6472:
    selected = []
    for key in ('names', 'names', 'english', 'english', 'templates'):
        state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
        values = corpus[key]
        selected.append(values[(state >> 8) % len(values)])
    first, second, opening, closing, template = selected
    paragraph = template.format(first=first, second=second, entry=len(parts), opening=opening, closing=closing) + '\n\n'
    parts.append(paragraph)
    characters += len(paragraph)

text = ''.join(parts)[:6472]

checksum = 0
word_count = 0

for size in (512, 2048, len(text)):
    sample = text[:size]
    in_word = False

    for index in range(len(sample)):
        char = sample[index]
        checksum += ord(char)

        if char.isspace():
            in_word = False
        elif not in_word:
            word_count += 1
            in_word = True

assert (word_count, checksum) == (1465, 842630)
