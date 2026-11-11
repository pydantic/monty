# /// monty
# [bench]
# data = '../data/string_corpus.json'
# ///
"""Run 4,096 random character reads per size in mostly-ASCII prose.

Repeat 256 seeded offsets 16 times on 512/2,048/6,472-character inputs; includes slicing, indexing and ord().
The harness times setup (JSON, text and offsets), the workload and assertions.
"""

import json

# Setup: decode the corpus and generate reproducible text.
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

# Setup: use an independent RNG stream to generate 256 lookup offsets.
state = 123
offsets = []

for _ in range(256):
    state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
    offsets.append(state >> 8)

# Benchmark: repeat the 256-offset pass 16 times for 4,096 reads per prefix.
checksum = 0

for size in (512, 2048, len(text)):
    sample = text[:size]
    length = len(sample)

    for repeat in range(16):
        for offset in offsets:
            checksum += ord(sample[offset % length])

# Validation: included in the timed execution.
assert checksum == 1157008
