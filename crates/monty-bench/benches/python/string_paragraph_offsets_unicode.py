# /// monty
# [bench]
# data = '../data/string_corpus.json'
# ///
"""Build paragraph start/end offsets in a 131,072-character, mostly-ASCII document.

Exercises successive find() calls with advancing starts and list/tuple construction.
The harness times setup (JSON, text generation and expansion), the workload and assertions.
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

# Benchmark: locate each paragraph and retain its start/end offsets.
length = len(text)
start = 0
boundaries = []

while start < length:
    end = text.find('\n\n', start)
    if end < 0:
        end = length
    boundaries.append((start, end))
    start = end + 2

# Validation: included in the timed execution.
assert length == 131072
assert len(boundaries) == 708
assert sum(end - start for start, end in boundaries) == 129658
assert sum(start for start, end in boundaries) == 46372291
