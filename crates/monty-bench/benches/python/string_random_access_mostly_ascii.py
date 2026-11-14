# /// monty
# [bench]
# data = '../data/string_corpus.json'
# ///
"""Run 4,096 random character reads in 128K characters of mostly-ASCII prose.

Repeat 256 seeded offsets 16 times; exercises character indexing and ord().
The harness times setup (JSON, text generation and expansion, and offsets), the workload and assertions.
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

# Setup: expand the generated prose into a 128K-character document.
copies = 131072 // len(text) + 1
text = '\n\n'.join(f'Notebook entry {number}\n\n{text}' for number in range(copies))[:131072]

# Setup: use an independent RNG stream to generate 256 lookup offsets.
state = 123
offsets = []

for _ in range(256):
    state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
    offsets.append(state >> 8)

# Benchmark: repeat the 256-offset pass 16 times for 4,096 reads.
length = len(text)
checksum = 0

for repeat in range(16):
    for offset in offsets:
        checksum += ord(text[offset % length])

# Validation: included in the timed execution.
assert len(text) == 131072
assert checksum == 383440
